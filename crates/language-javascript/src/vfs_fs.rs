//! `oxc_resolver::FileSystem` adapter backed by a `vfs::VfsPath`.
//!
//! The stock `oxc_resolver::Resolver` reads tsconfig, `package.json`, and
//! resolution targets directly from the real disk via `FileSystemOs`.
//! When the workspace root only exists virtually, that default
//! can't see any of those files. This adapter wires the resolver to the
//! same `VfsPath` the rest of the semantic provider already uses, so a
//! tsconfig fetched into the VFS is observed by tsconfig discovery,
//! `extends` chains, path aliases, and existence checks on resolved
//! targets.

use std::io;
use std::path::{Path, PathBuf};

use oxc_resolver::{FileMetadata, FileSystem as OxcFileSystem, ResolveError};
use vfs::error::VfsErrorKind;
use vfs::{VfsError, VfsFileType, VfsPath};

const NEW_PANIC_MSG: &str = "VfsFileSystem requires an explicit VfsPath root; \
     construct with VfsFileSystem::with_root(..) and pass it \
     to ResolverGeneric::new_with_file_system";

#[derive(Clone)]
pub struct VfsFileSystem {
    root: VfsPath,
}

impl VfsFileSystem {
    pub fn with_root(root: VfsPath) -> Self {
        Self { root }
    }

    /// Map an absolute path handed in by the resolver to a `VfsPath`
    /// under our root.
    ///
    /// oxc_resolver canonicalizes specifiers relative to the workspace
    /// root, which on POSIX yields `/app/foo/bar.ts` and on Windows
    /// yields `C:\app\foo\bar.ts`. Both have to collapse to the same
    /// `app/foo/bar.ts` VFS key; anything that assumes a single forward
    /// slash as the root marker will silently mis-resolve the Windows
    /// case (`VfsPath::join` rejects the leading `C:` prefix).
    ///
    /// Walking `Path::components()` drops drive prefixes, root markers,
    /// and `.` components, and puts each remaining `Normal` component
    /// under a `/`-joined relative key — exactly what `VfsPath::join`
    /// wants on both platforms.
    fn resolve_path(&self, path: &Path) -> io::Result<VfsPath> {
        let rel = path_to_vfs_rel(path);
        if rel.is_empty() {
            return Ok(self.root.clone());
        }
        self.root
            .join(&rel)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))
    }
}

/// Platform-portable conversion of an absolute/relative `Path` into the
/// `/`-joined relative string VFS paths expect. Intermediate `..`
/// components are preserved so the resolver can climb out of a
/// directory; drive prefixes, root markers, and `.` components are
/// dropped because the VFS has no concept of them.
fn path_to_vfs_rel(path: &Path) -> String {
    use std::path::Component;
    let mut out = String::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(&part.to_string_lossy());
            }
            Component::ParentDir => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str("..");
            }
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
        }
    }
    out
}

impl OxcFileSystem for VfsFileSystem {
    #[cfg(feature = "yarn_pnp")]
    fn new(_yarn_pnp: bool) -> Self {
        panic!("{}", NEW_PANIC_MSG)
    }

    #[cfg(not(feature = "yarn_pnp"))]
    fn new() -> Self {
        panic!("{}", NEW_PANIC_MSG)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let vp = self.resolve_path(path)?;
        let mut reader = vp.open_file().map_err(vfs_err_to_io)?;
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut reader, &mut buf)?;
        Ok(buf)
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        let vp = self.resolve_path(path)?;
        let meta = vp.metadata().map_err(vfs_err_to_io)?;
        Ok(match meta.file_type {
            VfsFileType::File => FileMetadata::new(true, false, false),
            VfsFileType::Directory => FileMetadata::new(false, true, false),
        })
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        // The VFS backends we ship with (MemoryFS, PhysicalFS) don't expose
        // symlink information separately from metadata. Treat every entry
        // as non-symlink — this is accurate for MemoryFS, and for
        // PhysicalFS it matches how oxc_resolver already walks by reading
        // symlink-resolved metadata.
        self.metadata(path)
    }

    fn read_link(&self, _path: &Path) -> Result<PathBuf, ResolveError> {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "vfs has no symlinks").into())
    }
}

fn vfs_err_to_io(err: VfsError) -> io::Error {
    match err.kind() {
        VfsErrorKind::FileNotFound => io::Error::from(io::ErrorKind::NotFound),
        VfsErrorKind::InvalidPath => io::Error::new(io::ErrorKind::InvalidInput, err.to_string()),
        _ => io::Error::other(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs::{MemoryFS, VfsPath};

    fn seed_memory_fs() -> VfsPath {
        let root: VfsPath = MemoryFS::new().into();
        root.join("app/src").unwrap().create_dir_all().unwrap();
        {
            let f = root.join("app/tsconfig.json").unwrap();
            let mut w = f.create_file().unwrap();
            use std::io::Write;
            w.write_all(b"{\"compilerOptions\":{}}").unwrap();
        }
        {
            let f = root.join("app/src/index.ts").unwrap();
            let mut w = f.create_file().unwrap();
            use std::io::Write;
            w.write_all(b"export const x = 1;").unwrap();
        }
        root
    }

    #[test]
    fn reads_absolute_path_via_root() {
        let root = seed_memory_fs();
        let fs = VfsFileSystem::with_root(root);
        let content = fs.read_to_string(Path::new("/app/tsconfig.json")).unwrap();
        assert!(content.contains("compilerOptions"));
    }

    #[test]
    fn metadata_reports_file_and_dir() {
        let root = seed_memory_fs();
        let fs = VfsFileSystem::with_root(root);

        let file_meta = fs.metadata(Path::new("/app/src/index.ts")).unwrap();
        assert!(file_meta.is_file());
        assert!(!file_meta.is_dir());

        let dir_meta = fs.metadata(Path::new("/app/src")).unwrap();
        assert!(dir_meta.is_dir());
        assert!(!dir_meta.is_file());
    }

    #[test]
    fn missing_file_returns_not_found() {
        let root = seed_memory_fs();
        let fs = VfsFileSystem::with_root(root);
        let err = fs.read_to_string(Path::new("/app/nope.ts")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}

//! Finding executables on `PATH`.

use std::path::{Path, PathBuf};

/// The first `name` on `PATH` that is an executable file.
pub(crate) fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    which_in(name, &path)
}

fn which_in(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path).find_map(|dir| {
        candidates(&dir, name)
            .into_iter()
            .find(|candidate| is_executable(candidate))
    })
}

#[cfg(windows)]
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    std::iter::once(dir.join(name))
        .chain(
            extensions
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| dir.join(format!("{name}{}", ext.to_ascii_lowercase()))),
        )
        .collect()
}

#[cfg(not(windows))]
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    vec![dir.join(name)]
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn finds_only_executable_files() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        let tool = dir.path().join("tool");
        std::fs::write(&plain, "").unwrap();
        std::fs::write(&tool, "").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().as_os_str();
        assert_eq!(which_in("tool", path), Some(tool));
        assert_eq!(which_in("plain", path), None);
        assert_eq!(which_in("missing", path), None);
    }
}

//! Virtual filesystem abstraction for semantic providers.
//!
//! This module provides a unified interface for file system operations
//! using the `vfs` crate, allowing semantic providers to work with
//! either real filesystems or in-memory filesystems.
//!
//! # Example
//!
//! Using with real filesystem:
//! ```rust,ignore
//! use language_core::filesystem::{VfsPath, PhysicalFS};
//!
//! let root: VfsPath = PhysicalFS::new("/path/to/project").into();
//! let content = language_core::filesystem::read_to_string(&root.join("src/main.ts")?)?;
//! ```
//!
//! Using with in-memory filesystem:
//! ```rust,ignore
//! use language_core::filesystem::{VfsPath, MemoryFS};
//!
//! let root: VfsPath = MemoryFS::new().into();
//! let file = root.join("test.ts")?;
//! file.create_file()?.write_all(b"const x = 1;")?;
//! ```

use std::io::Read;
use std::path::Path;

// Re-export core vfs types
pub use vfs::{MemoryFS, PhysicalFS, VfsError, VfsPath, VfsResult};

/// Read a file to string from a VfsPath.
///
/// This is a convenience function that opens the file and reads its contents.
///
/// # Arguments
///
/// * `path` - The VfsPath to read from
///
/// # Returns
///
/// The file contents as a String, or a VfsError if the operation fails.
///
/// # Example
///
/// ```rust,ignore
/// use language_core::filesystem::{VfsPath, PhysicalFS, read_to_string};
///
/// let root: VfsPath = PhysicalFS::new(".").into();
/// let content = read_to_string(&root.join("Cargo.toml")?)?;
/// ```
pub fn read_to_string(path: &VfsPath) -> VfsResult<String> {
    let mut content = String::new();
    path.open_file()?.read_to_string(&mut content)?;
    Ok(content)
}

/// Create a VfsPath from a std::path::Path using PhysicalFS.
///
/// This is a convenience function to convert a standard Path to a VfsPath
/// backed by the real filesystem.
///
/// # Arguments
///
/// * `path` - The filesystem path to convert
///
/// # Returns
///
/// A VfsPath pointing to the given path on the real filesystem.
pub fn physical_path(path: &Path) -> VfsPath {
    PhysicalFS::new(path).into()
}

/// Create a VfsPath backed by an in-memory filesystem.
///
/// This is useful for testing or when running in environments
/// without real filesystem access (e.g., pg_ast_grep PostgreSQL extension).
///
/// # Returns
///
/// A VfsPath pointing to the root of a new in-memory filesystem.
pub fn memory_fs() -> VfsPath {
    MemoryFS::new().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_physical_fs_read() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let root = physical_path(dir.path());
        let vfs_path = root.join("test.txt").unwrap();
        let content = read_to_string(&vfs_path).unwrap();

        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_memory_fs_read_write() {
        let root = memory_fs();
        let file = root.join("test.txt").unwrap();

        // Write to file
        file.create_file()
            .unwrap()
            .write_all(b"hello memory")
            .unwrap();

        // Read back
        let content = read_to_string(&file).unwrap();
        assert_eq!(content, "hello memory");
    }

    #[test]
    fn test_memory_fs_nested_paths() {
        let root = memory_fs();

        // Create nested path
        root.join("src").unwrap().create_dir().unwrap();
        let file = root.join("src/main.ts").unwrap();
        file.create_file()
            .unwrap()
            .write_all(b"const x = 1;")
            .unwrap();

        // Read back
        let content = read_to_string(&file).unwrap();
        assert_eq!(content, "const x = 1;");
    }

    #[test]
    fn test_file_not_found() {
        let root = memory_fs();
        let result = read_to_string(&root.join("nonexistent.txt").unwrap());
        assert!(result.is_err());
    }
}

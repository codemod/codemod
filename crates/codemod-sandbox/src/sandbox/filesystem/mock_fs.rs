use super::traits::{FileSystem, WalkOptions};
use crate::sandbox::errors::FsError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Mock filesystem implementation using in-memory storage
///
/// This is useful for testing and WASM scenarios where real filesystem access
/// is not available or desired.
#[derive(Debug)]
pub struct MockFileSystem {
    files: RwLock<HashMap<PathBuf, String>>,
    directories: RwLock<HashMap<PathBuf, bool>>,
}

impl MockFileSystem {
    pub fn new() -> Self {
        Self {
            files: RwLock::new(HashMap::new()),
            directories: RwLock::new(HashMap::new()),
        }
    }

    /// Add a file to the mock filesystem
    pub fn add_file<P: AsRef<Path>, S: Into<String>>(&self, path: P, content: S) {
        let path = path.as_ref().to_path_buf();
        let content = content.into();

        // Add the file
        self.files.write().unwrap().insert(path.clone(), content);

        // Ensure parent directories exist
        let mut current = path.clone();
        while let Some(parent) = current.parent() {
            self.directories
                .write()
                .unwrap()
                .insert(parent.to_path_buf(), true);
            current = parent.to_path_buf();
        }
    }

    /// Add a directory to the mock filesystem
    pub fn add_directory<P: AsRef<Path>>(&self, path: P) {
        let path = path.as_ref().to_path_buf();
        self.directories.write().unwrap().insert(path.clone(), true);

        // Ensure parent directories exist
        let mut current = path;
        while let Some(parent) = current.parent() {
            self.directories
                .write()
                .unwrap()
                .insert(parent.to_path_buf(), true);
            current = parent.to_path_buf();
        }
    }

    /// Remove a file from the mock filesystem
    pub fn remove_file<P: AsRef<Path>>(&self, path: P) {
        let path = path.as_ref().to_path_buf();
        self.files.write().unwrap().remove(&path);
    }

    /// Remove a directory from the mock filesystem
    pub fn remove_directory<P: AsRef<Path>>(&self, path: P) {
        let path = path.as_ref().to_path_buf();
        self.directories.write().unwrap().remove(&path);
    }

    /// Clear all files and directories
    pub fn clear(&self) {
        self.files.write().unwrap().clear();
        self.directories.write().unwrap().clear();
    }

    /// Get all files in the filesystem (for debugging)
    pub fn list_files(&self) -> Vec<PathBuf> {
        self.files.read().unwrap().keys().cloned().collect()
    }
}

impl Default for MockFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for MockFileSystem {
    async fn read_to_string(&self, path: &Path) -> Result<String, FsError> {
        self.files
            .read()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| FsError::FileNotFound {
                path: path.to_path_buf(),
            })
    }

    async fn exists(&self, path: &Path) -> bool {
        self.files.read().unwrap().contains_key(path)
            || self.directories.read().unwrap().contains_key(path)
    }

    async fn is_file(&self, path: &Path) -> bool {
        self.files.read().unwrap().contains_key(path)
    }

    async fn is_dir(&self, path: &Path) -> bool {
        self.directories.read().unwrap().contains_key(path)
    }

    async fn canonicalize(&self, path: &Path) -> Result<PathBuf, FsError> {
        // For mock filesystem, we just return the path as-is
        // In a real implementation, you might want to resolve relative paths
        if self.exists(path).await {
            Ok(path.to_path_buf())
        } else {
            Err(FsError::CanonicalizationFailed {
                path: path.to_path_buf(),
            })
        }
    }

    async fn walk_dir(&self, path: &Path, options: WalkOptions) -> Result<Vec<PathBuf>, FsError> {
        if !self.is_dir(path).await {
            return Err(FsError::FileNotFound {
                path: path.to_path_buf(),
            });
        }

        let files = self.files.read().unwrap();
        let mut result = Vec::new();

        for file_path in files.keys() {
            // Check if the file is under the given directory
            if file_path.starts_with(path) {
                // Apply filtering based on options
                let relative_path = file_path.strip_prefix(path).unwrap();

                // Check hidden files
                if !options.include_hidden
                    && relative_path
                        .components()
                        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
                {
                    continue;
                }

                // Check max depth
                if let Some(max_depth) = options.max_depth {
                    if relative_path.components().count() > max_depth {
                        continue;
                    }
                }

                result.push(file_path.clone());
            }
        }

        result.sort();
        Ok(result)
    }
}

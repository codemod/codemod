use std::path::{Path, PathBuf};

/// Searches for a file or directory with the given name in ancestor directories.
///
/// Starting from the given `start_path`, this function walks up the directory tree
/// looking for a file or directory named `target_name`. It returns the first match found,
/// or None if no match is found (including when the root directory is reached).
///
/// # Arguments
///
/// * `start_path` - The path to start searching from
/// * `target_name` - The name of the file or directory to find
///
/// # Returns
///
/// * `Some(PathBuf)` - The path to the found file/directory
/// * `None` - If the file/directory was not found in any ancestor
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// // Search for .git directory starting from /foo/bar
/// if let Some(git_dir) = find_in_ancestors("/foo/bar", ".git") {
///     println!("Found .git at: {}", git_dir.display());
/// }
/// ```
pub(crate) fn find_in_ancestors<P: AsRef<Path>>(
    start_path: P,
    target_name: &str,
) -> Option<PathBuf> {
    let mut current = start_path.as_ref();

    loop {
        let candidate = current.join(target_name);

        if candidate.exists() {
            return Some(candidate);
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) => current = parent,
            None => return None, // Reached the root
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_find_in_current_directory() {
        let temp_dir = std::env::temp_dir().join("test_find_in_current");
        fs::create_dir_all(&temp_dir).unwrap();

        let target_file = temp_dir.join(".git");
        fs::create_dir(&target_file).unwrap();

        let result = find_in_ancestors(&temp_dir, ".git");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), target_file);

        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_find_in_parent_directory() {
        let temp_dir = std::env::temp_dir().join("test_find_in_parent");
        let nested_dir = temp_dir.join("nested");
        fs::create_dir_all(&nested_dir).unwrap();

        let target_file = temp_dir.join(".git");
        fs::create_dir(&target_file).unwrap();

        let result = find_in_ancestors(&nested_dir, ".git");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), target_file);

        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_not_found() {
        let temp_dir = std::env::temp_dir().join("test_not_found");
        fs::create_dir_all(&temp_dir).unwrap();

        let result = find_in_ancestors(&temp_dir, ".nonexistent_file");
        assert!(result.is_none());

        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_find_file_not_directory() {
        let temp_dir = std::env::temp_dir().join("test_find_file");
        fs::create_dir_all(&temp_dir).unwrap();

        let target_file = temp_dir.join("package.json");
        fs::write(&target_file, "{}").unwrap();

        let result = find_in_ancestors(&temp_dir, "package.json");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), target_file);

        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_deeply_nested() {
        let temp_dir = std::env::temp_dir().join("test_deeply_nested");
        let deep_dir = temp_dir.join("a").join("b").join("c").join("d");
        fs::create_dir_all(&deep_dir).unwrap();

        let target_file = temp_dir.join(".git");
        fs::create_dir(&target_file).unwrap();

        let result = find_in_ancestors(&deep_dir, ".git");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), target_file);

        fs::remove_dir_all(&temp_dir).unwrap();
    }
}

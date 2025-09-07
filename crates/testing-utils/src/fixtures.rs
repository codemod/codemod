use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Represents a single transformation test case with input and expected output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformationTestCase {
    pub name: String,
    pub input_code: String,
    pub expected_output_code: String,
}

/// Source of test cases - either from filesystem or provided directly
#[derive(Debug, Clone)]
pub enum TestSource {
    /// Test cases discovered from a directory structure
    Directory(PathBuf),
    /// Test cases provided directly as a vector
    Cases(Vec<TransformationTestCase>),
}

/// A test case discovered from the filesystem
#[derive(Debug, Clone)]
pub struct FileSystemTestCase {
    pub name: String,
    pub input_files: HashMap<PathBuf, TestFile>,
    pub expected_files: HashMap<PathBuf, TestFile>,
    pub path: PathBuf,
    pub should_error: bool,
}

/// A test file with its content and metadata
#[derive(Debug, Clone)]
pub struct TestFile {
    pub path: PathBuf,
    pub content: String,
    pub relative_path: PathBuf,
}

/// Unified test case that can represent both filesystem and direct test cases
#[derive(Debug, Clone)]
pub struct UnifiedTestCase {
    pub name: String,
    pub input_code: String,
    pub expected_output_code: String,
    pub should_error: bool,
    pub input_path: Option<PathBuf>,
    pub expected_output_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum TestError {
    #[error("No input file found in {test_dir}. Expected one of: {expected_extensions:?}")]
    NoInputFile {
        test_dir: PathBuf,
        expected_extensions: Vec<String>,
    },

    #[error("Multiple input files found in {test_dir}: {found_files:?}. {suggestion}")]
    AmbiguousInputFiles {
        test_dir: PathBuf,
        found_files: Vec<PathBuf>,
        suggestion: String,
    },

    #[error("No expected file found for {input_file} in {test_dir}")]
    NoExpectedFile {
        test_dir: PathBuf,
        input_file: PathBuf,
    },

    #[error("Invalid test structure in {0}")]
    InvalidTestStructure(PathBuf),

    #[error("Invalid test name for {0}")]
    InvalidTestName(PathBuf),

    #[error("Invalid file path: {0}")]
    InvalidFilePath(PathBuf),

    #[error("Cannot update snapshots for test '{test_name}' - it's not a filesystem-based test")]
    SnapshotUpdateNotSupported { test_name: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl TestSource {
    /// Convert test source to unified test cases
    /// The extensions parameter should be a list of file extensions to look for (e.g., [".js", ".ts"])
    pub fn to_unified_test_cases(
        &self,
        extensions: &[&str],
    ) -> Result<Vec<UnifiedTestCase>, TestError> {
        match self {
            TestSource::Directory(dir) => {
                let fs_test_cases = FileSystemTestCase::discover_in_directory(dir, extensions)?;
                let mut unified_cases = Vec::new();

                for fs_case in fs_test_cases {
                    let input_files_len = fs_case.input_files.len();
                    // For filesystem test cases, we need to handle multiple input/expected file pairs
                    // Handle cases where expected files might be missing (for --update-snapshots)
                    for (key, input_file) in fs_case.input_files {
                        let (expected_content, expected_path) =
                            match fs_case.expected_files.get(&key) {
                                Some(expected_file) => (
                                    expected_file.content.clone(),
                                    Some(expected_file.path.clone()),
                                ),
                                None => {
                                    // Expected file doesn't exist - create placeholder path for snapshot updates
                                    let input_path = input_file.path.to_string_lossy().to_string();
                                    let expected_path = input_path.replace("input", "expected");
                                    ("".to_string(), Some(PathBuf::from(expected_path)))
                                }
                            };

                        unified_cases.push(UnifiedTestCase {
                            name: if input_files_len > 1 {
                                format!("{}_{}", fs_case.name, input_file.relative_path.display())
                            } else {
                                fs_case.name.clone()
                            },
                            input_code: input_file.content.clone(),
                            expected_output_code: expected_content,
                            should_error: fs_case.should_error,
                            input_path: Some(input_file.path.clone()),
                            expected_output_path: expected_path,
                        });
                    }
                }

                Ok(unified_cases)
            }
            TestSource::Cases(cases) => {
                Ok(cases
                    .iter()
                    .map(|case| UnifiedTestCase {
                        name: case.name.clone(),
                        input_code: case.input_code.clone(),
                        expected_output_code: case.expected_output_code.clone(),
                        should_error: false, // Direct cases don't have error expectations by default
                        input_path: None,    // Direct cases don't have a file path
                        expected_output_path: None, // Direct cases don't have an expected output file
                    })
                    .collect())
            }
        }
    }
}

impl FileSystemTestCase {
    /// Discover all test cases in a directory
    pub fn discover_in_directory(
        test_dir: &Path,
        extensions: &[&str],
    ) -> Result<Vec<FileSystemTestCase>, TestError> {
        let mut test_cases = Vec::new();

        for entry in std::fs::read_dir(test_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                if let Ok(test_case) = Self::from_directory(&path, extensions) {
                    test_cases.push(test_case);
                }
            }
        }

        test_cases.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(test_cases)
    }

    /// Create a test case from a directory
    fn from_directory(
        test_dir: &Path,
        extensions: &[&str],
    ) -> Result<FileSystemTestCase, TestError> {
        let name = test_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| TestError::InvalidTestName(test_dir.to_path_buf()))?
            .to_string();

        // Determine if this test should expect errors based on naming convention
        let should_error = name.ends_with("_should_error");

        // Check for single file format (input.js + expected.js)
        if let Ok(input_files) = find_input_files(test_dir, extensions) {
            let expected_files = find_expected_files(test_dir, &input_files)?;

            return Ok(FileSystemTestCase {
                name,
                input_files: input_files
                    .into_iter()
                    .map(|path| TestFile::from_path(&path))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|file| (file.relative_path.clone(), file))
                    .collect::<HashMap<_, _>>(),
                expected_files: expected_files
                    .into_iter()
                    .map(|path| TestFile::from_path(&path))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|file| (file.relative_path.clone(), file))
                    .collect::<HashMap<_, _>>(),
                path: test_dir.to_path_buf(),
                should_error,
            });
        }

        // Check for multi-file format (input/ + expected/ directories)
        let input_dir = test_dir.join("input");
        let expected_dir = test_dir.join("expected");

        if input_dir.exists() && expected_dir.exists() {
            let input_files = collect_files_in_directory(&input_dir, extensions)?;
            let expected_files = collect_files_in_directory(&expected_dir, extensions)?;

            return Ok(FileSystemTestCase {
                name,
                input_files,
                expected_files,
                path: test_dir.to_path_buf(),
                should_error,
            });
        }

        Err(TestError::InvalidTestStructure(test_dir.to_path_buf()))
    }

    /// Check if expected files exist, or return an error that can be handled by --update-snapshots
    pub fn validate_expected_files(&self) -> Result<(), TestError> {
        if self.expected_files.is_empty() {
            // Return the first input file as context for the error
            if let Some((_, input_file)) = self.input_files.iter().next() {
                return Err(TestError::NoExpectedFile {
                    test_dir: self.path.clone(),
                    input_file: input_file.path.clone(),
                });
            }
        }
        Ok(())
    }
}

impl UnifiedTestCase {
    /// Check if this test case should expect errors (either from naming or explicit configuration)
    pub fn should_expect_error(&self, expect_error_patterns: &[String]) -> bool {
        // Check explicit patterns first
        let pattern_match = expect_error_patterns
            .iter()
            .any(|pattern| self.name.contains(pattern));

        // Fall back to naming convention or explicit should_error field
        pattern_match || self.should_error
    }

    /// Update the expected output file with new content (only works for filesystem-based tests)
    pub fn update_expected_output(&self, new_content: &str) -> Result<(), TestError> {
        if let Some(expected_path) = &self.expected_output_path {
            // Ensure the parent directory exists
            if let Some(parent) = expected_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(expected_path, new_content)?;
            Ok(())
        } else {
            // For direct/adhoc test cases, we can't update files
            Err(TestError::SnapshotUpdateNotSupported {
                test_name: self.name.clone(),
            })
        }
    }
}

impl TestFile {
    pub fn from_path(path: &Path) -> Result<TestFile, TestError> {
        let content = std::fs::read_to_string(path)?;
        let relative_path = path
            .file_name()
            .ok_or_else(|| TestError::InvalidFilePath(path.to_path_buf()))?
            .into();

        Ok(TestFile {
            path: path.to_path_buf(),
            content,
            relative_path,
        })
    }

    /// Create a TestFile from content (for creating expected files during snapshot updates)
    pub fn from_content(relative_path: PathBuf, content: String, base_dir: &Path) -> TestFile {
        let path = base_dir.join(&relative_path);
        TestFile {
            path,
            content,
            relative_path,
        }
    }

    /// Write the test file to disk (for snapshot updates)
    pub fn write_to_disk(&self) -> Result<(), TestError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, &self.content)?;
        Ok(())
    }
}

/// Find input files based on extensions
fn find_input_files(test_dir: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>, TestError> {
    let mut candidates = Vec::new();

    // Look for input.{ext} files
    for ext in extensions {
        let input_file = test_dir.join(format!("input{ext}"));
        if input_file.exists() {
            candidates.push(input_file);
        }
    }

    match candidates.len() {
        0 => Err(TestError::NoInputFile {
            test_dir: test_dir.to_path_buf(),
            expected_extensions: extensions.iter().map(|s| s.to_string()).collect(),
        }),
        1 => Ok(candidates),
        _ => Err(TestError::AmbiguousInputFiles {
            test_dir: test_dir.to_path_buf(),
            found_files: candidates,
            suggestion: "Use only one input file per test case, or organize into input/ and expected/ directories".to_string(),
        }),
    }
}

/// Find expected files corresponding to input files
fn find_expected_files(
    test_dir: &Path,
    input_files: &[PathBuf],
) -> Result<Vec<PathBuf>, TestError> {
    let mut expected_files = Vec::new();

    for input_file in input_files {
        let input_name = input_file
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| TestError::InvalidFilePath(input_file.clone()))?;

        // Replace "input" with "expected" in the filename
        let expected_name = input_name.replace("input", "expected");
        let expected_file = test_dir.join(expected_name);

        if expected_file.exists() {
            expected_files.push(expected_file);
        } else {
            // Don't error here - let the caller handle missing expected files
            // This allows --update-snapshots to work
        }
    }

    Ok(expected_files)
}

/// Collect files in a directory that match the extensions
fn collect_files_in_directory(
    dir: &Path,
    extensions: &[&str],
) -> Result<HashMap<PathBuf, TestFile>, TestError> {
    let mut files = HashMap::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            // Check if the file has a matching extension
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_with_dot = format!(".{ext}");
                if extensions.contains(&ext_with_dot.as_str()) {
                    if let Ok(file) = TestFile::from_path(&path) {
                        files.insert(file.relative_path.clone(), file);
                    }
                }
            }
        }
    }

    Ok(files)
}

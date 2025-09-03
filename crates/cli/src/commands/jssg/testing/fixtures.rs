use anyhow::Result;
use codemod_ast_grep_dynamic_lang::DynamicLang;
use codemod_sandbox::sandbox::engine::language_data::get_extensions_for_language;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TestCase {
    pub name: String,
    pub input_files: Vec<TestFile>,
    pub expected_files: Vec<TestFile>,
    pub path: PathBuf,
    pub should_error: bool,
}

#[derive(Debug, Clone)]
pub struct TestFile {
    pub path: PathBuf,
    pub content: String,
    pub relative_path: PathBuf,
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

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl TestCase {
    /// Discover all test cases in a directory
    pub fn discover_in_directory(
        test_dir: &Path,
        language: DynamicLang,
    ) -> Result<Vec<TestCase>, TestError> {
        let mut test_cases = Vec::new();

        for entry in std::fs::read_dir(test_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                if let Ok(test_case) = Self::from_directory(&path, language) {
                    test_cases.push(test_case);
                }
            }
        }

        test_cases.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(test_cases)
    }

    /// Create a test case from a directory
    fn from_directory(test_dir: &Path, language: DynamicLang) -> Result<TestCase, TestError> {
        let name = test_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| TestError::InvalidTestName(test_dir.to_path_buf()))?
            .to_string();

        // Determine if this test should expect errors based on naming convention
        let should_error = name.ends_with("_should_error");

        // Check for single file format (input.js + expected.js)
        if let Ok(input_files) = find_input_files(test_dir, language) {
            let expected_files = find_expected_files(test_dir, &input_files)?;

            return Ok(TestCase {
                name,
                input_files: input_files
                    .into_iter()
                    .map(|path| TestFile::from_path(&path))
                    .collect::<Result<Vec<_>, _>>()?,
                expected_files: expected_files
                    .into_iter()
                    .map(|path| TestFile::from_path(&path))
                    .collect::<Result<Vec<_>, _>>()?,
                path: test_dir.to_path_buf(),
                should_error,
            });
        }

        // Check for multi-file format (input/ + expected/ directories)
        let input_dir = test_dir.join("input");
        let expected_dir = test_dir.join("expected");

        if input_dir.exists() && expected_dir.exists() {
            let input_files = collect_files_in_directory(&input_dir, language)?;
            let expected_files = collect_files_in_directory(&expected_dir, language)?;

            return Ok(TestCase {
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
            if let Some(input_file) = self.input_files.first() {
                return Err(TestError::NoExpectedFile {
                    test_dir: self.path.clone(),
                    input_file: input_file.path.clone(),
                });
            }
        }
        Ok(())
    }

    /// Check if this test case should expect errors (either from naming or explicit configuration)
    pub fn should_expect_error(&self, expect_error_patterns: &[String]) -> bool {
        // Check explicit patterns first
        let pattern_match = expect_error_patterns
            .iter()
            .any(|pattern| self.name.contains(pattern));

        // Fall back to naming convention or explicit should_error field
        pattern_match || self.should_error
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

/// Find input files based on language extensions
fn find_input_files(test_dir: &Path, language: DynamicLang) -> Result<Vec<PathBuf>, TestError> {
    let extensions = get_extensions_for_language(language.name());
    let mut candidates = Vec::new();

    // Look for input.{ext} files
    for ext in &extensions {
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

/// Collect files in a directory that match the language extensions
fn collect_files_in_directory(
    dir: &Path,
    language: DynamicLang,
) -> Result<Vec<TestFile>, TestError> {
    let extensions = get_extensions_for_language(language.name());
    let mut files = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            // Check if the file has a matching extension
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_with_dot = format!(".{ext}");
                if extensions.contains(&ext_with_dot.as_str()) {
                    files.push(TestFile::from_path(&path)?);
                }
            }
        }
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

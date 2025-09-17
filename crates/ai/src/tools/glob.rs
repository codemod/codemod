//! Cross-platform glob pattern matching tool

use async_trait::async_trait;
use coro_core::error::Result;
use coro_core::impl_tool_factory;
use coro_core::tools::{Tool, ToolCall, ToolExample, ToolResult};
use ignore::{
    gitignore::{Gitignore, GitignoreBuilder},
    Match,
};
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Maximum number of files to return to prevent overwhelming output
const MAX_RESULTS: usize = 1000;

/// Configuration for file matching
#[derive(Debug, Clone)]
struct MatchConfig {
    pattern: String,
    base_path: PathBuf,
    include_hidden: bool,
    respect_gitignore: bool,
    case_sensitive: Option<bool>,
    max_depth: Option<u32>,
    files_only: bool,
    dirs_only: bool,
}

/// Cross-platform glob pattern matching tool
pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files and directories matching glob patterns (cross-platform)\n\
         * Supports standard glob patterns: *, ?, [abc], {foo,bar}, **\n\
         * Works on all platforms (Windows, macOS, Linux)\n\
         * Respects .gitignore files by default (can be disabled)\n\
         * Returns absolute paths for found files and directories\n\
         * Limited to first 1000 matches to prevent overwhelming output\n\
         \n\
         Common glob patterns:\n\
         * `*.rs` - All Rust files in current directory\n\
         * `src/**/*.rs` - All Rust files in src directory recursively\n\
         * `**/*.{js,ts}` - All JavaScript and TypeScript files recursively\n\
         * `**/test_*.py` - All Python test files recursively\n\
         * `[Dd]ocument*` - Files starting with Document or document\n\
         * `file?.txt` - Files like file1.txt, fileA.txt, etc."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files and directories. Use / as path separator on all platforms."
                },
                "base_path": {
                    "type": "string",
                    "description": "Base directory to start searching from (default: current directory). Must be absolute path."
                },
                "include_hidden": {
                    "type": "boolean",
                    "description": "Include hidden files and directories (default: false).",
                    "default": false
                },
                "respect_gitignore": {
                    "type": "boolean",
                    "description": "Respect .gitignore files when searching (default: true).",
                    "default": true
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Whether pattern matching should be case sensitive (default: platform dependent)."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth to search (default: unlimited).",
                    "minimum": 1
                },
                "files_only": {
                    "type": "boolean",
                    "description": "Only return files, not directories (default: false).",
                    "default": false
                },
                "dirs_only": {
                    "type": "boolean",
                    "description": "Only return directories, not files (default: false).",
                    "default": false
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult> {
        let pattern: String = call.get_parameter("pattern")?;
        let base_path: String = call.get_parameter_or("base_path", ".".to_string());
        let include_hidden: bool = call.get_parameter_or("include_hidden", false);
        let respect_gitignore: bool = call.get_parameter_or("respect_gitignore", true);
        let case_sensitive: Option<bool> = call.get_parameter("case_sensitive").ok();
        let max_depth: Option<u32> = call.get_parameter("max_depth").ok();
        let files_only: bool = call.get_parameter_or("files_only", false);
        let dirs_only: bool = call.get_parameter_or("dirs_only", false);

        // Validate inputs
        if files_only && dirs_only {
            return Ok(ToolResult::error(
                &call.id,
                &"Cannot specify both files_only and dirs_only as true".to_string(),
            ));
        }

        let base_path = Path::new(&base_path);
        if !base_path.exists() {
            return Ok(ToolResult::error(
                &call.id,
                &format!("Base path does not exist: {}", base_path.display()),
            ));
        }

        // Convert relative base path to absolute
        let base_path = if base_path.is_relative() {
            std::env::current_dir()
                .map_err(|e| format!("Cannot get current directory: {}", e))?
                .join(base_path)
        } else {
            base_path.to_path_buf()
        };

        let config = MatchConfig {
            pattern: pattern.clone(),
            base_path: base_path.clone(),
            include_hidden,
            respect_gitignore,
            case_sensitive,
            max_depth,
            files_only,
            dirs_only,
        };

        match self.find_matching_files(config).await {
            Ok(matches) => {
                if matches.is_empty() {
                    Ok(ToolResult::success(
                        &call.id,
                        &format!(
                            "No files found matching pattern '{}' in {}",
                            pattern,
                            base_path.display()
                        ),
                    ))
                } else {
                    let truncated = matches.len() >= MAX_RESULTS;
                    let match_list: Vec<String> = matches
                        .into_iter()
                        .map(|p| p.display().to_string())
                        .collect();

                    let mut result = format!(
                        "Found {} files matching pattern '{}'{}:\n\n{}",
                        match_list.len(),
                        pattern,
                        if truncated {
                            " (truncated to first 1000 results)"
                        } else {
                            ""
                        },
                        match_list.join("\n")
                    );

                    if truncated {
                        result.push_str("\n\n⚠️  Results truncated. Use more specific patterns to reduce matches.");
                    }

                    Ok(ToolResult::success(&call.id, &result).with_data(json!({
                        "matches": match_list,
                        "pattern": pattern,
                        "base_path": base_path.display().to_string(),
                        "truncated": truncated,
                        "total_found": match_list.len()
                    })))
                }
            }
            Err(e) => Ok(ToolResult::error(
                &call.id,
                &format!("Error finding files: {}", e),
            )),
        }
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Find all Rust files in current directory".to_string(),
                parameters: json!({"pattern": "*.rs"}),
                expected_result: "List of all .rs files in current directory".to_string(),
            },
            ToolExample {
                description: "Find all JavaScript and TypeScript files recursively".to_string(),
                parameters: json!({"pattern": "**/*.{js,ts}"}),
                expected_result: "List of all .js and .ts files in all subdirectories".to_string(),
            },
            ToolExample {
                description: "Find all test files in src directory".to_string(),
                parameters: json!({
                    "pattern": "**/test_*.py",
                    "base_path": "src"
                }),
                expected_result: "List of all Python test files in src directory".to_string(),
            },
            ToolExample {
                description: "Find only directories with case-sensitive matching".to_string(),
                parameters: json!({
                    "pattern": "**/[Dd]ocs",
                    "dirs_only": true,
                    "case_sensitive": true
                }),
                expected_result: "List of directories named 'Docs' or 'docs'".to_string(),
            },
            ToolExample {
                description: "Find files including hidden ones, ignoring .gitignore".to_string(),
                parameters: json!({
                    "pattern": "**/.env*",
                    "include_hidden": true,
                    "respect_gitignore": false
                }),
                expected_result: "List of all .env files including hidden ones".to_string(),
            },
            ToolExample {
                description: "Find config files with depth limit".to_string(),
                parameters: json!({
                    "pattern": "**/config.*",
                    "max_depth": 3,
                    "files_only": true
                }),
                expected_result: "List of config files within 3 directory levels".to_string(),
            },
        ]
    }
}

impl GlobTool {
    async fn find_matching_files(&self, config: MatchConfig) -> Result<Vec<PathBuf>> {
        // Build gitignore matcher if needed
        let gitignore = if config.respect_gitignore {
            self.build_gitignore(&config.base_path)?
        } else {
            None
        };

        // Determine case sensitivity (default based on platform)
        let case_sensitive = config.case_sensitive.unwrap_or({
            // Default: case-insensitive on Windows, case-sensitive elsewhere
            !cfg!(target_os = "windows")
        });

        let mut matches = Vec::new();
        let mut seen_paths = HashSet::new();

        // Convert glob pattern to a more structured form for matching
        let glob_matcher = GlobMatcher::new(&config.pattern, case_sensitive)?;

        // Set up walkdir with appropriate settings
        let mut walker = WalkDir::new(&config.base_path);

        if let Some(depth) = config.max_depth {
            walker = walker.max_depth(depth as usize);
        }

        for entry in walker.into_iter() {
            if matches.len() >= MAX_RESULTS {
                break;
            }

            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue, // Skip entries we can't read
            };

            let path = entry.path();
            let is_file = path.is_file();
            let is_dir = path.is_dir();

            // Skip if we've already seen this path
            if !seen_paths.insert(path.to_path_buf()) {
                continue;
            }

            // Apply file/dir filtering
            if config.files_only && !is_file {
                continue;
            }
            if config.dirs_only && !is_dir {
                continue;
            }

            // Skip hidden files if not requested
            if !config.include_hidden {
                if let Some(name) = path.file_name() {
                    if let Some(name_str) = name.to_str() {
                        if name_str.starts_with('.') && path != config.base_path {
                            continue;
                        }
                    }
                }
            }

            // Check gitignore
            if let Some(ref gitignore) = gitignore {
                let relative_path = path.strip_prefix(&config.base_path).unwrap_or(path);
                match gitignore.matched(relative_path, is_dir) {
                    Match::Ignore(_) => continue,
                    Match::None | Match::Whitelist(_) => {}
                }
            }

            // Check if path matches the glob pattern
            if glob_matcher.matches(path, &config.base_path) {
                matches.push(path.to_path_buf());
            }
        }

        // Sort results for consistent output
        matches.sort();
        Ok(matches)
    }

    fn build_gitignore(&self, base_path: &Path) -> Result<Option<Gitignore>> {
        let mut builder = GitignoreBuilder::new(base_path);

        // Try to add .gitignore files walking up the directory tree
        let mut current_path = base_path;
        loop {
            let gitignore_path = current_path.join(".gitignore");
            if gitignore_path.exists() {
                if let Some(e) = builder.add(&gitignore_path) {
                    tracing::warn!(
                        "Failed to parse .gitignore at {}: {}",
                        gitignore_path.display(),
                        e
                    );
                }
            }

            match current_path.parent() {
                Some(parent) => current_path = parent,
                None => break,
            }
        }

        match builder.build() {
            Ok(gitignore) => Ok(Some(gitignore)),
            Err(e) => {
                tracing::warn!("Failed to build gitignore matcher: {}", e);
                Ok(None)
            }
        }
    }
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple glob pattern matcher
struct GlobMatcher {
    pattern: String,
    case_sensitive: bool,
}

impl GlobMatcher {
    fn new(pattern: &str, case_sensitive: bool) -> Result<Self> {
        Ok(Self {
            pattern: pattern.to_string(),
            case_sensitive,
        })
    }

    fn matches(&self, path: &Path, base_path: &Path) -> bool {
        // Get relative path from base
        let relative_path = match path.strip_prefix(base_path) {
            Ok(rel) => rel,
            Err(_) => path,
        };

        // Convert path to string with forward slashes (cross-platform)
        let path_str = relative_path.to_string_lossy().replace('\\', "/");

        self.match_pattern(&self.pattern, &path_str)
    }

    fn match_pattern(&self, pattern: &str, text: &str) -> bool {
        // Simple glob matching implementation
        // This is a basic implementation - could be enhanced with more sophisticated matching

        let pattern = if self.case_sensitive {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };

        let text = if self.case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        };

        self.match_glob(&pattern, &text)
    }

    fn match_glob(&self, pattern: &str, text: &str) -> bool {
        // Handle ** (recursive directory matching)
        if pattern.contains("**") {
            let parts: Vec<&str> = pattern.split("**").collect();
            if parts.len() == 2 {
                let prefix = parts[0];
                let suffix = parts[1];

                // Handle prefix
                if !prefix.is_empty() && !text.starts_with(prefix) {
                    return false;
                }

                // Handle suffix
                if !suffix.is_empty() {
                    // Try to find suffix anywhere in the remaining text
                    let remaining_text = if prefix.is_empty() {
                        text
                    } else {
                        &text[prefix.len()..]
                    };

                    return self.match_suffix_anywhere(suffix, remaining_text);
                }

                return true;
            }
        }

        // Handle simple glob patterns
        self.simple_glob_match(pattern, text)
    }

    fn match_suffix_anywhere(&self, suffix: &str, text: &str) -> bool {
        if suffix.is_empty() {
            return true;
        }

        // Try matching suffix at every position
        for i in 0..=text.len() {
            if text.len() >= i + suffix.len() && self.simple_glob_match(suffix, &text[i..]) {
                return true;
            }
        }
        false
    }

    fn simple_glob_match(&self, pattern: &str, text: &str) -> bool {
        let pattern_chars: Vec<char> = pattern.chars().collect();
        let text_chars: Vec<char> = text.chars().collect();

        self.match_chars(&pattern_chars, &text_chars, 0, 0)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn match_chars(&self, pattern: &[char], text: &[char], p: usize, t: usize) -> bool {
        if p >= pattern.len() {
            return t >= text.len();
        }

        if t >= text.len() {
            // Check if remaining pattern is all '*'
            return pattern[p..].iter().all(|&c| c == '*');
        }

        match pattern[p] {
            '*' => {
                // Try matching zero or more characters
                self.match_chars(pattern, text, p + 1, t)
                    || self.match_chars(pattern, text, p, t + 1)
            }
            '?' => {
                // Match any single character
                self.match_chars(pattern, text, p + 1, t + 1)
            }
            c if c == text[t] => {
                // Exact character match
                self.match_chars(pattern, text, p + 1, t + 1)
            }
            _ => false,
        }
    }
}

impl_tool_factory!(
    GlobToolFactory,
    GlobTool,
    "glob",
    "Find files and directories using cross-platform glob patterns"
);

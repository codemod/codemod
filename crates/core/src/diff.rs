use similar::{ChangeTag, TextDiff};
use std::path::Path;

/// Configuration for diff generation
#[derive(Clone, Debug)]
pub struct DiffConfig {
    /// Number of context lines to show around changes (default: 3)
    pub context_lines: usize,
    /// Whether to output colored diff (respects NO_COLOR env)
    pub color: bool,
    /// Maximum lines per file diff (0 = unlimited, default: 500)
    pub max_lines_per_file: usize,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            context_lines: 3,
            color: std::env::var("NO_COLOR").is_err(),
            max_lines_per_file: 500,
        }
    }
}

impl DiffConfig {
    /// Create a DiffConfig with explicit color control.
    ///
    /// Color is disabled if `no_color` is true OR if the `NO_COLOR` env var is set.
    pub fn with_color_control(no_color: bool) -> Self {
        Self {
            color: !no_color && std::env::var("NO_COLOR").is_err(),
            ..Self::default()
        }
    }
}

/// Result of generating a diff for a file
#[derive(Clone, Debug)]
pub struct FileDiff {
    /// Path to the file
    pub path: String,
    /// The unified diff text
    pub diff_text: String,
    /// Number of lines added
    pub additions: usize,
    /// Number of lines deleted
    pub deletions: usize,
}

impl FileDiff {
    /// Print the diff to stdout with standard formatting.
    ///
    /// Output format:
    /// ```text
    /// ============================================================
    /// File: /path/to/file.rs
    /// ============================================================
    /// --- [before] /path/to/file.rs
    /// +++ [after]  /path/to/file.rs
    /// @@ ... @@
    /// ...diff content...
    /// +N additions, -M deletions
    /// ```
    pub fn print(&self) {
        println!("\n{}", "=".repeat(60));
        println!("File: {}", self.path);
        println!("{}", "=".repeat(60));
        print!("{}", self.diff_text);
        println!("+{} additions, -{} deletions", self.additions, self.deletions);
    }
}

/// Generate a unified diff between original and modified content
pub fn generate_unified_diff(
    file_path: &Path,
    original: &str,
    modified: &str,
    config: &DiffConfig,
) -> FileDiff {
    let diff = TextDiff::from_lines(original, modified);
    let path_str = file_path.display().to_string();

    let mut diff_text = String::new();
    let mut additions = 0;
    let mut deletions = 0;
    let mut line_count = 0;

    // Build unified diff header
    let header_a = format!("--- [before] {}", path_str);
    let header_b = format!("+++ [after]  {}", path_str);

    if config.color {
        diff_text.push_str(&format!("\x1b[1m{}\x1b[0m\n", header_a));
        diff_text.push_str(&format!("\x1b[1m{}\x1b[0m\n", header_b));
    } else {
        diff_text.push_str(&format!("{}\n", header_a));
        diff_text.push_str(&format!("{}\n", header_b));
    }

    // Generate unified diff with context
    for hunk in diff.unified_diff().context_radius(config.context_lines).iter_hunks() {
        // Add hunk header
        let hunk_header = hunk.header().to_string();
        if config.color {
            diff_text.push_str(&format!("\x1b[36m{}\x1b[0m", hunk_header));
        } else {
            diff_text.push_str(&hunk_header);
        }

        for change in hunk.iter_changes() {
            if config.max_lines_per_file > 0 && line_count >= config.max_lines_per_file {
                diff_text.push_str("\n... (diff truncated)\n");
                break;
            }

            let sign = match change.tag() {
                ChangeTag::Delete => {
                    deletions += 1;
                    "-"
                }
                ChangeTag::Insert => {
                    additions += 1;
                    "+"
                }
                ChangeTag::Equal => " ",
            };

            let line = change.to_string_lossy();
            let line_output = format!("{}{}", sign, line);

            if config.color {
                let colored_line = match change.tag() {
                    ChangeTag::Delete => format!("\x1b[31m{}\x1b[0m", line_output),
                    ChangeTag::Insert => format!("\x1b[32m{}\x1b[0m", line_output),
                    ChangeTag::Equal => line_output,
                };
                diff_text.push_str(&colored_line);
            } else {
                diff_text.push_str(&line_output);
            }

            // Add newline if the line doesn't end with one
            if !line.ends_with('\n') {
                diff_text.push('\n');
            }

            line_count += 1;
        }
    }

    FileDiff {
        path: path_str,
        diff_text,
        additions,
        deletions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_generate_unified_diff_basic() {
        let original = "line1\nline2\nline3\n";
        let modified = "line1\nmodified\nline3\n";
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(&path, original, modified, &config);

        assert_eq!(result.additions, 1);
        assert_eq!(result.deletions, 1);
        assert!(result.diff_text.contains("-line2"));
        assert!(result.diff_text.contains("+modified"));
    }

    #[test]
    fn test_generate_unified_diff_additions_only() {
        let original = "line1\nline2\n";
        let modified = "line1\nline2\nline3\n";
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(&path, original, modified, &config);

        assert_eq!(result.additions, 1);
        assert_eq!(result.deletions, 0);
        assert!(result.diff_text.contains("+line3"));
    }

    #[test]
    fn test_generate_unified_diff_deletions_only() {
        let original = "line1\nline2\nline3\n";
        let modified = "line1\nline2\n";
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(&path, original, modified, &config);

        assert_eq!(result.additions, 0);
        assert_eq!(result.deletions, 1);
        assert!(result.diff_text.contains("-line3"));
    }

    #[test]
    fn test_generate_unified_diff_no_changes() {
        let original = "line1\nline2\n";
        let modified = "line1\nline2\n";
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(&path, original, modified, &config);

        assert_eq!(result.additions, 0);
        assert_eq!(result.deletions, 0);
    }

    #[test]
    fn test_generate_unified_diff_with_color() {
        let original = "line1\n";
        let modified = "line2\n";
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: true,
            ..Default::default()
        };

        let result = generate_unified_diff(&path, original, modified, &config);

        // Check that ANSI color codes are present
        assert!(result.diff_text.contains("\x1b[31m")); // Red for deletions
        assert!(result.diff_text.contains("\x1b[32m")); // Green for additions
    }

    #[test]
    fn test_generate_unified_diff_truncation() {
        let original = (0..1000).map(|i| format!("line{}\n", i)).collect::<String>();
        let modified = (0..1000)
            .map(|i| format!("modified{}\n", i))
            .collect::<String>();
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: false,
            max_lines_per_file: 10,
            ..Default::default()
        };

        let result = generate_unified_diff(&path, &original, &modified, &config);

        assert!(result.diff_text.contains("(diff truncated)"));
    }
}

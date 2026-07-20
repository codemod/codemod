use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

/// The kind of filesystem change a [`FileDiff`] represents.
///
/// Defaults to `Modified` so existing callers that only ever produced
/// content diffs keep their previous behavior without any changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    /// File content changed at the same path.
    #[default]
    Modified,
    /// File is new (it did not exist before this change).
    Added,
    /// File was removed.
    Deleted,
    /// File was renamed and/or moved to a new path. `old_path` on
    /// [`FileDiff`] holds the original location.
    Renamed,
}

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
    /// Path to the file. For renames/moves this is the *new* path.
    pub path: String,
    /// The unified diff text
    pub diff_text: String,
    /// Number of lines added
    pub additions: usize,
    /// Number of lines deleted
    pub deletions: usize,
    /// Step identifier that produced the change
    pub step_id: Option<String>,
    /// Human-readable step name that produced the change
    pub step_name: Option<String>,
    /// Parent step identifier used for report grouping
    pub parent_step_id: Option<String>,
    /// Parent step name used for report grouping
    pub parent_step_name: Option<String>,
    /// What kind of filesystem change this diff represents.
    pub kind: ChangeKind,
    /// For renames/moves, the original path before the change.
    pub old_path: Option<String>,
}

/// Optional workflow metadata associated with a generated diff.
#[derive(Clone, Debug, Default)]
pub struct DiffMetadata {
    pub step_id: Option<String>,
    pub step_name: Option<String>,
    pub parent_step_id: Option<String>,
    pub parent_step_name: Option<String>,
    /// What kind of filesystem change this diff represents.
    pub kind: ChangeKind,
    /// For renames/moves, the original path before the change.
    pub old_path: Option<std::path::PathBuf>,
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
        println!(
            "+{} additions, -{} deletions",
            self.additions, self.deletions
        );
    }
}

/// Generate a unified diff between original and modified content
pub fn generate_unified_diff(
    file_path: &Path,
    original: &str,
    modified: &str,
    config: &DiffConfig,
    metadata: DiffMetadata,
) -> FileDiff {
    let diff = TextDiff::from_lines(original, modified);
    let path_str = file_path.display().to_string();
    let old_path_str = metadata.old_path.as_ref().map(|p| p.display().to_string());

    let mut diff_text = String::new();
    let mut additions = 0;
    let mut deletions = 0;
    let mut line_count = 0;

    // Build unified diff header. Structural changes (rename/add/delete) get
    // an explicit annotation up front so the change is clear even before any
    // content hunks are rendered (e.g. a rename with no content changes
    // would otherwise show an empty, misleading diff body).
    let (header_a, header_b) = match metadata.kind {
        ChangeKind::Renamed => {
            let old = old_path_str.clone().unwrap_or_else(|| path_str.clone());
            diff_text.push_str(&format!("Renamed: {} -> {}\n", old, path_str));
            (
                format!("--- [before] {}", old),
                format!("+++ [after]  {}", path_str),
            )
        }
        ChangeKind::Added => {
            diff_text.push_str(&format!("Added: {}\n", path_str));
            (
                "--- [before] /dev/null".to_string(),
                format!("+++ [after]  {}", path_str),
            )
        }
        ChangeKind::Deleted => {
            diff_text.push_str(&format!("Deleted: {}\n", path_str));
            (
                format!("--- [before] {}", path_str),
                "+++ [after]  /dev/null".to_string(),
            )
        }
        ChangeKind::Modified => (
            format!("--- [before] {}", path_str),
            format!("+++ [after]  {}", path_str),
        ),
    };

    if config.color {
        diff_text.push_str(&format!("\x1b[1m{}\x1b[0m\n", header_a));
        diff_text.push_str(&format!("\x1b[1m{}\x1b[0m\n", header_b));
    } else {
        diff_text.push_str(&format!("{}\n", header_a));
        diff_text.push_str(&format!("{}\n", header_b));
    }

    // Generate unified diff with context
    for hunk in diff
        .unified_diff()
        .context_radius(config.context_lines)
        .iter_hunks()
    {
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
        step_id: metadata.step_id,
        step_name: metadata.step_name,
        parent_step_id: metadata.parent_step_id,
        parent_step_name: metadata.parent_step_name,
        kind: metadata.kind,
        old_path: old_path_str,
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

        let result =
            generate_unified_diff(&path, original, modified, &config, DiffMetadata::default());

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

        let result =
            generate_unified_diff(&path, original, modified, &config, DiffMetadata::default());

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

        let result =
            generate_unified_diff(&path, original, modified, &config, DiffMetadata::default());

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

        let result =
            generate_unified_diff(&path, original, modified, &config, DiffMetadata::default());

        assert_eq!(result.additions, 0);
        assert_eq!(result.deletions, 0);
    }

    #[test]
    fn test_generate_unified_diff_preserves_metadata() {
        let original = "line1\n";
        let modified = "line2\n";
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(
            &path,
            original,
            modified,
            &config,
            DiffMetadata {
                step_id: Some("step-1".to_string()),
                step_name: Some("Step 1".to_string()),
                parent_step_id: Some("parent-1".to_string()),
                parent_step_name: Some("Parent Step".to_string()),
                ..DiffMetadata::default()
            },
        );

        assert_eq!(result.step_id.as_deref(), Some("step-1"));
        assert_eq!(result.step_name.as_deref(), Some("Step 1"));
        assert_eq!(result.parent_step_id.as_deref(), Some("parent-1"));
        assert_eq!(result.parent_step_name.as_deref(), Some("Parent Step"));
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

        let result =
            generate_unified_diff(&path, original, modified, &config, DiffMetadata::default());

        // Check that ANSI color codes are present
        assert!(result.diff_text.contains("\x1b[31m")); // Red for deletions
        assert!(result.diff_text.contains("\x1b[32m")); // Green for additions
    }

    #[test]
    fn test_generate_unified_diff_truncation() {
        let original = (0..1000)
            .map(|i| format!("line{}\n", i))
            .collect::<String>();
        let modified = (0..1000)
            .map(|i| format!("modified{}\n", i))
            .collect::<String>();
        let path = PathBuf::from("test.txt");
        let config = DiffConfig {
            color: false,
            max_lines_per_file: 10,
            ..Default::default()
        };

        let result = generate_unified_diff(
            &path,
            &original,
            &modified,
            &config,
            DiffMetadata::default(),
        );

        assert!(result.diff_text.contains("(diff truncated)"));
    }

    #[test]
    fn test_generate_unified_diff_defaults_to_modified_kind() {
        let result = generate_unified_diff(
            &PathBuf::from("test.txt"),
            "a\n",
            "b\n",
            &DiffConfig::default(),
            DiffMetadata::default(),
        );

        assert_eq!(result.kind, ChangeKind::Modified);
        assert_eq!(result.old_path, None);
    }

    #[test]
    fn test_generate_unified_diff_renamed_marks_kind_and_old_path() {
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(
            &PathBuf::from("new/name.ts"),
            "const a = 1;\n",
            "const a = 1;\n",
            &config,
            DiffMetadata {
                kind: ChangeKind::Renamed,
                old_path: Some(PathBuf::from("old/name.ts")),
                ..DiffMetadata::default()
            },
        );

        assert_eq!(result.kind, ChangeKind::Renamed);
        assert_eq!(result.old_path.as_deref(), Some("old/name.ts"));
        assert_eq!(result.path, "new/name.ts");
        assert!(result
            .diff_text
            .contains("Renamed: old/name.ts -> new/name.ts"));
    }

    #[test]
    fn test_generate_unified_diff_added_marks_kind() {
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(
            &PathBuf::from("new-file.ts"),
            "",
            "export const a = 1;\n",
            &config,
            DiffMetadata {
                kind: ChangeKind::Added,
                ..DiffMetadata::default()
            },
        );

        assert_eq!(result.kind, ChangeKind::Added);
        assert_eq!(result.additions, 1);
        assert_eq!(result.deletions, 0);
        assert!(result.diff_text.contains("Added: new-file.ts"));
        assert!(result.diff_text.contains("/dev/null"));
    }

    #[test]
    fn test_generate_unified_diff_deleted_marks_kind() {
        let config = DiffConfig {
            color: false,
            ..Default::default()
        };

        let result = generate_unified_diff(
            &PathBuf::from("gone.ts"),
            "export const a = 1;\n",
            "",
            &config,
            DiffMetadata {
                kind: ChangeKind::Deleted,
                ..DiffMetadata::default()
            },
        );

        assert_eq!(result.kind, ChangeKind::Deleted);
        assert_eq!(result.additions, 0);
        assert_eq!(result.deletions, 1);
        assert!(result.diff_text.contains("Deleted: gone.ts"));
        assert!(result.diff_text.contains("/dev/null"));
    }
}

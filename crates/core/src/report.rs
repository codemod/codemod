use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Statistics about the codemod execution
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportStats {
    pub files_modified: usize,
    pub files_unmodified: usize,
    pub files_with_errors: usize,
    pub total_additions: usize,
    pub total_deletions: usize,
}

/// A single metric entry with cardinality dimensions and count
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportMetricEntry {
    pub cardinality: HashMap<String, String>,
    pub count: u64,
}

/// A file diff entry in the report
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportFileDiff {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_text: Option<String>,
    pub additions: usize,
    pub deletions: usize,
}

/// What to include when sharing a report
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ShareLevel {
    /// Only stats and metrics — no file list
    MetricsOnly,
    /// Stats, metrics, and file paths with +/- counts — no diff text
    WithFiles,
}

/// The full execution report for a codemod run
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionReport {
    /// Schema version for future evolution
    pub version: u32,
    /// Unique report identifier
    pub id: String,
    /// Name of the codemod that was run
    pub codemod_name: String,
    /// Version of the codemod (if from registry)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codemod_version: Option<String>,
    /// ISO 8601 timestamp of execution
    pub executed_at: String,
    /// Execution duration in milliseconds
    pub duration_ms: f64,
    /// Whether this was a dry run
    pub dry_run: bool,
    /// Target path the codemod ran against
    pub target_path: String,
    /// CLI version
    pub cli_version: String,
    /// Operating system
    pub os: String,
    /// CPU architecture
    pub arch: String,
    /// Execution statistics
    pub stats: ReportStats,
    /// Metrics collected during execution (metric_name -> entries)
    pub metrics: HashMap<String, Vec<ReportMetricEntry>>,
    /// File diffs from the execution
    pub diffs: Vec<ReportFileDiff>,
}

impl ExecutionReport {
    #[allow(clippy::too_many_arguments)]
    /// Build a new execution report
    pub fn build(
        codemod_name: String,
        codemod_version: Option<String>,
        duration_ms: f64,
        dry_run: bool,
        target_path: String,
        cli_version: String,
        files_modified: usize,
        files_unmodified: usize,
        files_with_errors: usize,
        metrics: HashMap<String, Vec<ReportMetricEntry>>,
        diffs: Vec<ReportFileDiff>,
    ) -> Self {
        let total_additions: usize = diffs.iter().map(|d| d.additions).sum();
        let total_deletions: usize = diffs.iter().map(|d| d.deletions).sum();

        Self {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            codemod_name,
            codemod_version,
            executed_at: chrono::Utc::now().to_rfc3339(),
            duration_ms,
            dry_run,
            target_path,
            cli_version,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            stats: ReportStats {
                files_modified,
                files_unmodified,
                files_with_errors,
                total_additions,
                total_deletions,
            },
            metrics,
            diffs,
        }
    }

    /// Create a copy of this report stripped for sharing.
    ///
    /// - Diff text is always removed (never sent to the cloud).
    /// - `MetricsOnly` also removes the file list entirely.
    /// - `WithFiles` keeps file paths and +/- counts.
    /// - `target_path` is cleared so local disk paths aren't shared.
    pub fn strip_for_sharing(&self, level: &ShareLevel) -> Self {
        let mut stripped = self.clone();

        // Never share local disk paths
        stripped.target_path = String::new();

        match level {
            ShareLevel::MetricsOnly => {
                stripped.diffs = Vec::new();
            }
            ShareLevel::WithFiles => {
                stripped.diffs = stripped
                    .diffs
                    .into_iter()
                    .map(|d| ReportFileDiff {
                        path: d.path,
                        diff_text: None,
                        additions: d.additions,
                        deletions: d.deletions,
                    })
                    .collect();
            }
        }

        stripped
    }
}

/// Convert MetricsData (from codemod-sandbox) into report-compatible format
pub fn convert_metrics(
    metrics_data: &HashMap<String, Vec<codemod_sandbox::metrics::MetricEntry>>,
) -> HashMap<String, Vec<ReportMetricEntry>> {
    metrics_data
        .iter()
        .map(|(name, entries)| {
            let report_entries = entries
                .iter()
                .map(|e| ReportMetricEntry {
                    cardinality: e.cardinality.clone(),
                    count: e.count,
                })
                .collect();
            (name.clone(), report_entries)
        })
        .collect()
}

/// Convert a list of FileDiffs into report-compatible format.
///
/// Paths are made relative to `target_path` so absolute disk paths
/// are never stored in the report.
pub fn convert_diffs(diffs: &[crate::diff::FileDiff], target_path: &str) -> Vec<ReportFileDiff> {
    let base = Path::new(target_path);
    diffs
        .iter()
        .map(|d| {
            let rel = normalize_report_diff_path(Path::new(&d.path), base);
            ReportFileDiff {
                path: rel,
                diff_text: Some(d.diff_text.clone()),
                additions: d.additions,
                deletions: d.deletions,
            }
        })
        .collect()
}

fn normalize_report_diff_path(diff_path: &Path, target_path: &Path) -> String {
    if let Ok(relative) = diff_path.strip_prefix(target_path) {
        return relative.display().to_string();
    }

    if let Some(relative) = relativize_managed_worktree_path(diff_path) {
        return relative.display().to_string();
    }

    diff_path.display().to_string()
}

fn relativize_managed_worktree_path(diff_path: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = diff_path.components().collect();
    let worktree_root_index = components.iter().position(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .ends_with(".codemod-worktrees")
    })?;

    let relative_components = components
        .into_iter()
        .skip(worktree_root_index + 2)
        .collect::<Vec<_>>();
    if relative_components.is_empty() {
        return None;
    }

    let mut relative_path = PathBuf::new();
    for component in relative_components {
        relative_path.push(component.as_os_str());
    }
    Some(relative_path)
}

#[cfg(test)]
mod tests {
    use super::{convert_diffs, normalize_report_diff_path};
    use crate::diff::FileDiff;
    use std::path::Path;

    #[test]
    fn convert_diffs_keeps_paths_relative_to_target() {
        let diffs = vec![FileDiff {
            path: "/tmp/repo/src/app.ts".to_string(),
            diff_text: "@@".to_string(),
            additions: 1,
            deletions: 0,
        }];

        let report_diffs = convert_diffs(&diffs, "/tmp/repo");
        assert_eq!(report_diffs[0].path, "src/app.ts");
    }

    #[test]
    fn convert_diffs_relativizes_managed_worktree_paths() {
        let diffs = vec![FileDiff {
            path: "/Users/me/backstage.codemod-worktrees/codemod-123/src/plugins/catalog.ts"
                .to_string(),
            diff_text: "@@".to_string(),
            additions: 4,
            deletions: 2,
        }];

        let report_diffs = convert_diffs(&diffs, "/Users/me/backstage");
        assert_eq!(report_diffs[0].path, "src/plugins/catalog.ts");
    }

    #[test]
    fn normalize_report_diff_path_preserves_unrelated_paths() {
        let path = Path::new("/outside/project/generated.txt");
        let target = Path::new("/tmp/repo");

        assert_eq!(
            normalize_report_diff_path(path, target),
            "/outside/project/generated.txt"
        );
    }
}

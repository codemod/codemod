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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
}

/// A group of file diffs produced by a specific step
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDiffGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub step_name: String,
    pub additions: usize,
    pub deletions: usize,
    pub diffs: Vec<ReportFileDiff>,
}

/// Aggregated diff payload used by the report
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDiffs {
    pub consolidated: Vec<ReportFileDiff>,
    pub by_step: Vec<ReportDiffGroup>,
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
    /// File diffs from the execution, consolidated by file path
    pub diffs: Vec<ReportFileDiff>,
    /// File diffs grouped by the step that produced them
    pub diff_groups: Vec<ReportDiffGroup>,
    /// Registry URL opened by the report UI Registry button
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_link_url: Option<String>,
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
        report_diffs: ReportDiffs,
    ) -> Self {
        let total_additions: usize = report_diffs.consolidated.iter().map(|d| d.additions).sum();
        let total_deletions: usize = report_diffs.consolidated.iter().map(|d| d.deletions).sum();

        Self {
            version: 2,
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
            diffs: report_diffs.consolidated,
            diff_groups: report_diffs.by_step,
            registry_link_url: None,
        }
    }

    pub fn with_registry_link_url(mut self, registry_link_url: Option<String>) -> Self {
        self.registry_link_url = registry_link_url;
        self
    }

    /// Create a copy of this report stripped for sharing.
    ///
    /// - Diff text is always removed (never sent to the cloud).
    /// - `MetricsOnly` also removes the file list entirely.
    /// - `WithFiles` keeps file paths and +/- counts.
    /// - Shared reports never keep step-group metadata; they are consolidated-only to avoid
    ///   sharing workflow structure and step naming details unnecessarily.
    /// - `target_path` is cleared so local disk paths aren't shared.
    pub fn strip_for_sharing(&self, level: &ShareLevel) -> Self {
        let mut stripped = self.clone();

        // Never share local disk paths
        stripped.target_path = String::new();

        match level {
            ShareLevel::MetricsOnly => {
                stripped.diffs = Vec::new();
                stripped.diff_groups = Vec::new();
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
                        step_id: None,
                        step_name: None,
                    })
                    .collect();
                stripped.diff_groups = Vec::new();
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
pub fn convert_diffs(diffs: &[crate::diff::FileDiff], target_path: &str) -> ReportDiffs {
    let base = Path::new(target_path);
    let mut consolidated: Vec<ReportFileDiff> = Vec::new();
    let mut consolidated_index: HashMap<String, usize> = HashMap::new();
    let mut by_step: Vec<ReportDiffGroup> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();

    for diff in diffs {
        let rel = normalize_report_diff_path(Path::new(&diff.path), base);
        let report_diff = ReportFileDiff {
            path: rel.clone(),
            diff_text: Some(diff.diff_text.clone()),
            additions: diff.additions,
            deletions: diff.deletions,
            step_id: diff.step_id.clone(),
            step_name: diff.step_name.clone(),
        };

        if let Some(&index) = consolidated_index.get(&rel) {
            let existing: &mut ReportFileDiff = &mut consolidated[index];
            existing.additions += report_diff.additions;
            existing.deletions += report_diff.deletions;
            existing.diff_text = Some(merge_diff_text(
                existing.diff_text.as_deref(),
                report_diff.diff_text.as_deref(),
                report_diff
                    .step_name
                    .as_deref()
                    .or(report_diff.step_id.as_deref()),
            ));
        } else {
            consolidated_index.insert(rel.clone(), consolidated.len());
            consolidated.push(ReportFileDiff {
                diff_text: Some(merge_diff_text(
                    None,
                    report_diff.diff_text.as_deref(),
                    report_diff
                        .step_name
                        .as_deref()
                        .or(report_diff.step_id.as_deref()),
                )),
                step_id: None,
                step_name: None,
                ..report_diff.clone()
            });
        }

        let group_step_id = normalize_optional_step_value(diff.parent_step_id.clone())
            .or_else(|| normalize_optional_step_value(diff.step_id.clone()));
        let group_step_name = normalize_optional_step_value(diff.parent_step_name.clone())
            .or_else(|| normalize_optional_step_value(diff.step_name.clone()));

        let step_key = group_step_id.clone().unwrap_or_else(|| {
            group_step_name
                .clone()
                .unwrap_or_else(|| "ungrouped".to_string())
        });
        let step_name = group_step_name
            .clone()
            .or(group_step_id.clone())
            .unwrap_or_else(|| "Ungrouped".to_string());

        if let Some(&index) = group_index.get(&step_key) {
            let group = &mut by_step[index];
            group.additions += report_diff.additions;
            group.deletions += report_diff.deletions;
            group.diffs.push(report_diff);
        } else {
            group_index.insert(step_key, by_step.len());
            by_step.push(ReportDiffGroup {
                step_id: group_step_id,
                step_name,
                additions: report_diff.additions,
                deletions: report_diff.deletions,
                diffs: vec![report_diff],
            });
        }
    }

    ReportDiffs {
        consolidated,
        by_step,
    }
}

fn merge_diff_text(existing: Option<&str>, next: Option<&str>, step_label: Option<&str>) -> String {
    let Some(next) = next else {
        return existing.unwrap_or_default().to_string();
    };
    let next_text = if let Some(step_label) = step_label {
        format!("Step: {step_label}\n{next}")
    } else {
        next.to_string()
    };

    match existing {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{next_text}"),
        _ => next_text,
    }
}

fn normalize_optional_step_value(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.len() == value.len() {
            Some(value)
        } else {
            Some(trimmed.to_string())
        }
    })
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
    use super::{
        convert_diffs, normalize_report_diff_path, ExecutionReport, ReportDiffGroup, ReportDiffs,
        ReportFileDiff,
    };
    use crate::diff::FileDiff;
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn convert_diffs_keeps_paths_relative_to_target() {
        let diffs = vec![FileDiff {
            path: "/tmp/repo/src/app.ts".to_string(),
            diff_text: "@@".to_string(),
            additions: 1,
            deletions: 0,
            step_id: None,
            step_name: None,
            parent_step_id: None,
            parent_step_name: None,
        }];

        let report_diffs = convert_diffs(&diffs, "/tmp/repo");
        assert_eq!(report_diffs.consolidated[0].path, "src/app.ts");
    }

    #[test]
    fn convert_diffs_relativizes_managed_worktree_paths() {
        let diffs = vec![FileDiff {
            path: "/Users/me/backstage.codemod-worktrees/codemod-123/src/plugins/catalog.ts"
                .to_string(),
            diff_text: "@@".to_string(),
            additions: 4,
            deletions: 2,
            step_id: None,
            step_name: None,
            parent_step_id: None,
            parent_step_name: None,
        }];

        let report_diffs = convert_diffs(&diffs, "/Users/me/backstage");
        assert_eq!(report_diffs.consolidated[0].path, "src/plugins/catalog.ts");
    }

    #[test]
    fn convert_diffs_consolidates_repeated_paths_and_groups_by_step() {
        let diffs = vec![
            FileDiff {
                path: "/tmp/repo/src/app.ts".to_string(),
                diff_text: "@@ first".to_string(),
                additions: 1,
                deletions: 0,
                step_id: Some("step-1".to_string()),
                step_name: Some("First Step".to_string()),
                parent_step_id: None,
                parent_step_name: None,
            },
            FileDiff {
                path: "/tmp/repo/src/app.ts".to_string(),
                diff_text: "@@ second".to_string(),
                additions: 2,
                deletions: 1,
                step_id: Some("step-2".to_string()),
                step_name: Some("Second Step".to_string()),
                parent_step_id: None,
                parent_step_name: None,
            },
        ];

        let report_diffs = convert_diffs(&diffs, "/tmp/repo");

        assert_eq!(report_diffs.consolidated.len(), 1);
        assert_eq!(report_diffs.consolidated[0].path, "src/app.ts");
        assert_eq!(report_diffs.consolidated[0].additions, 3);
        assert_eq!(report_diffs.consolidated[0].deletions, 1);
        assert!(report_diffs.consolidated[0]
            .diff_text
            .as_deref()
            .is_some_and(|text| text.contains("Step: First Step")));
        assert_eq!(report_diffs.by_step.len(), 2);
        assert_eq!(report_diffs.by_step[0].step_name, "First Step");
        assert_eq!(report_diffs.by_step[1].step_name, "Second Step");
    }

    #[test]
    fn convert_diffs_does_not_collapse_distinct_steps_with_empty_step_ids() {
        let diffs = vec![
            FileDiff {
                path: "/tmp/repo/src/app.ts".to_string(),
                diff_text: "@@ first".to_string(),
                additions: 1,
                deletions: 0,
                step_id: Some(String::new()),
                step_name: Some("First Step".to_string()),
                parent_step_id: None,
                parent_step_name: None,
            },
            FileDiff {
                path: "/tmp/repo/src/app.ts".to_string(),
                diff_text: "@@ second".to_string(),
                additions: 2,
                deletions: 1,
                step_id: Some("".to_string()),
                step_name: Some("Second Step".to_string()),
                parent_step_id: None,
                parent_step_name: None,
            },
        ];

        let report_diffs = convert_diffs(&diffs, "/tmp/repo");

        assert_eq!(report_diffs.by_step.len(), 2);
        assert_eq!(report_diffs.by_step[0].step_name, "First Step");
        assert_eq!(report_diffs.by_step[1].step_name, "Second Step");
        assert_eq!(report_diffs.by_step[0].step_id, None);
        assert_eq!(report_diffs.by_step[1].step_id, None);
    }

    #[test]
    fn convert_diffs_groups_nested_codemod_changes_by_parent_step() {
        let diffs = vec![
            FileDiff {
                path: "/tmp/repo/src/app.ts".to_string(),
                diff_text: "@@ first".to_string(),
                additions: 1,
                deletions: 0,
                step_id: Some("nested-1".to_string()),
                step_name: Some("Nested Step One".to_string()),
                parent_step_id: Some("codemod-1".to_string()),
                parent_step_name: Some("Outer Codemod".to_string()),
            },
            FileDiff {
                path: "/tmp/repo/src/other.ts".to_string(),
                diff_text: "@@ second".to_string(),
                additions: 2,
                deletions: 1,
                step_id: Some("nested-2".to_string()),
                step_name: Some("Nested Step Two".to_string()),
                parent_step_id: Some("codemod-1".to_string()),
                parent_step_name: Some("Outer Codemod".to_string()),
            },
        ];

        let report_diffs = convert_diffs(&diffs, "/tmp/repo");

        assert_eq!(report_diffs.by_step.len(), 1);
        assert_eq!(
            report_diffs.by_step[0].step_id.as_deref(),
            Some("codemod-1")
        );
        assert_eq!(report_diffs.by_step[0].step_name, "Outer Codemod");
        assert_eq!(report_diffs.by_step[0].diffs.len(), 2);
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

    #[test]
    fn build_preserves_workflow_files_modified_count() {
        let report = ExecutionReport::build(
            "demo".to_string(),
            None,
            12.0,
            true,
            "/tmp/repo".to_string(),
            "1.0.0".to_string(),
            4,
            2,
            1,
            HashMap::new(),
            ReportDiffs {
                consolidated: vec![ReportFileDiff {
                    path: "src/app.ts".to_string(),
                    diff_text: Some("@@".to_string()),
                    additions: 3,
                    deletions: 1,
                    step_id: None,
                    step_name: None,
                }],
                by_step: vec![ReportDiffGroup {
                    step_id: None,
                    step_name: "Step".to_string(),
                    additions: 3,
                    deletions: 1,
                    diffs: vec![],
                }],
            },
        );

        assert_eq!(report.stats.files_modified, 4);
        assert_eq!(report.stats.total_additions, 3);
        assert_eq!(report.stats.total_deletions, 1);
    }
}

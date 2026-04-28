use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub(crate) const DEFAULT_WORKFLOW_NAME: &str = "default";
pub(crate) const DEFAULT_WORKFLOW_FILE: &str = "workflow.yaml";

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct CodemodManifest {
    pub(crate) schema_version: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) description: String,
    pub(crate) author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) copyright: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bugs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) registry: Option<RegistryConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workflows: Option<Vec<WorkflowEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) targets: Option<TargetConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) keywords: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) readme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) changelog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) validation: Option<ValidationConfig>,
    pub(crate) capabilities: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct WorkflowEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) default: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct RegistryConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) access: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) visibility: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct TargetConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    languages: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frameworks: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    versions: Option<std::collections::HashMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct ValidationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_tests: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_test_coverage: Option<u32>,
}

impl CodemodManifest {
    /// Returns the canonical list of workflows declared by this manifest.
    /// Lenient: tolerates legacy + new fields coexisting, mixed-case names,
    /// and multiple `default: true` entries. Used at run/validate time so
    /// already-published packages with imperfect manifests still execute.
    /// Use `validate_workflow_entries` for strict publish-time checks.
    pub(crate) fn resolved_workflows(&self) -> Result<Vec<WorkflowEntry>> {
        match (&self.workflow, &self.workflows) {
            (_, Some(entries)) if !entries.is_empty() => {
                normalize_workflow_entries_lenient(entries)
            }
            (Some(path), _) => {
                let trimmed = path.trim();
                let resolved_path = if trimmed.is_empty() {
                    DEFAULT_WORKFLOW_FILE.to_string()
                } else {
                    trimmed.to_string()
                };
                Ok(vec![WorkflowEntry {
                    name: DEFAULT_WORKFLOW_NAME.to_string(),
                    path: resolved_path,
                    description: None,
                    default: true,
                }])
            }
            _ => Ok(vec![WorkflowEntry {
                name: DEFAULT_WORKFLOW_NAME.to_string(),
                path: DEFAULT_WORKFLOW_FILE.to_string(),
                description: None,
                default: true,
            }]),
        }
    }

    /// Strict validation for publish time. Returns the first error found.
    pub(crate) fn validate_workflow_entries(&self) -> Result<()> {
        if self.workflow.is_some() && self.workflows.as_ref().is_some_and(|e| !e.is_empty()) {
            return Err(anyhow!(
                "codemod.yaml cannot set both `workflow` and `workflows`. Use one or the other."
            ));
        }
        if let Some(entries) = &self.workflows {
            normalize_workflow_entries_strict(entries)?;
        }
        Ok(())
    }

    /// Returns the workflow that should run when no explicit choice is made.
    pub(crate) fn default_workflow(&self) -> Result<WorkflowEntry> {
        let workflows = self.resolved_workflows()?;
        Ok(workflows
            .into_iter()
            .find(|entry| entry.default)
            .expect("resolved_workflows always returns at least one default entry"))
    }

    /// Resolves a workflow by name, or the default if `name` is `None`.
    pub(crate) fn find_workflow(&self, name: Option<&str>) -> Result<WorkflowEntry> {
        let workflows = self.resolved_workflows()?;
        match name {
            None => Ok(workflows
                .into_iter()
                .find(|entry| entry.default)
                .expect("resolved_workflows always returns at least one default entry")),
            Some(requested) => {
                if let Some(entry) = workflows.iter().find(|entry| entry.name == requested) {
                    return Ok(entry.clone());
                }
                let available: Vec<String> = workflows.into_iter().map(|e| e.name).collect();
                Err(anyhow!(
                    "Workflow `{}` not found in codemod.yaml. Available: {}",
                    requested,
                    available.join(", ")
                ))
            }
        }
    }
}

fn normalize_workflow_entries_strict(entries: &[WorkflowEntry]) -> Result<Vec<WorkflowEntry>> {
    if entries.is_empty() {
        return Err(anyhow!("`workflows` must contain at least one entry"));
    }

    let mut seen_names = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut default_count = 0;

    for entry in entries {
        let trimmed_name = entry.name.trim();
        if trimmed_name.is_empty() {
            return Err(anyhow!("Workflow entry has empty `name`."));
        }
        if !is_valid_workflow_name(trimmed_name) {
            return Err(anyhow!(
                "Workflow name `{}` is invalid. Names must contain only ASCII letters, digits, `-`, or `_`.",
                trimmed_name
            ));
        }
        if !seen_names.insert(trimmed_name.to_string()) {
            return Err(anyhow!(
                "Duplicate workflow name `{}` in codemod.yaml `workflows`.",
                trimmed_name
            ));
        }

        let trimmed_path = entry.path.trim();
        if trimmed_path.is_empty() {
            return Err(anyhow!("Workflow `{}` has empty `path`.", trimmed_name));
        }
        if !seen_paths.insert(trimmed_path.to_string()) {
            return Err(anyhow!(
                "Duplicate workflow path `{}` in codemod.yaml `workflows`.",
                trimmed_path
            ));
        }

        if entry.default {
            default_count += 1;
        }
    }

    if default_count > 1 {
        return Err(anyhow!(
            "Multiple workflows are marked `default: true` in codemod.yaml. Only one is allowed."
        ));
    }

    let mut result: Vec<WorkflowEntry> = entries
        .iter()
        .map(|entry| WorkflowEntry {
            name: entry.name.trim().to_string(),
            path: entry.path.trim().to_string(),
            description: entry
                .description
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            default: entry.default,
        })
        .collect();

    if default_count == 0 {
        // Implicit default: first entry.
        if let Some(first) = result.first_mut() {
            first.default = true;
        }
    }

    Ok(result)
}

fn normalize_workflow_entries_lenient(entries: &[WorkflowEntry]) -> Result<Vec<WorkflowEntry>> {
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut result: Vec<WorkflowEntry> = Vec::new();

    for entry in entries {
        let trimmed_name = entry.name.trim();
        let trimmed_path = entry.path.trim();
        if trimmed_name.is_empty() || trimmed_path.is_empty() {
            continue;
        }
        if !seen_names.insert(trimmed_name.to_string()) {
            continue;
        }
        if !seen_paths.insert(trimmed_path.to_string()) {
            continue;
        }
        result.push(WorkflowEntry {
            name: trimmed_name.to_string(),
            path: trimmed_path.to_string(),
            description: entry
                .description
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            default: entry.default,
        });
    }

    if result.is_empty() {
        return Ok(vec![WorkflowEntry {
            name: DEFAULT_WORKFLOW_NAME.to_string(),
            path: DEFAULT_WORKFLOW_FILE.to_string(),
            description: None,
            default: true,
        }]);
    }

    // Keep at most one default; if multiple were flagged, retain only the
    // first. If none were flagged, the first entry becomes the default.
    let mut seen_default = false;
    for entry in result.iter_mut() {
        if entry.default {
            if seen_default {
                entry.default = false;
            } else {
                seen_default = true;
            }
        }
    }
    if !seen_default {
        if let Some(first) = result.first_mut() {
            first.default = true;
        }
    }

    Ok(result)
}

fn is_valid_workflow_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_manifest() -> CodemodManifest {
        CodemodManifest {
            schema_version: "1.0".to_string(),
            name: "example".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            license: None,
            copyright: None,
            repository: None,
            homepage: None,
            bugs: None,
            registry: None,
            workflow: None,
            workflows: None,
            targets: None,
            dependencies: None,
            keywords: None,
            category: None,
            readme: None,
            changelog: None,
            documentation: None,
            validation: None,
            capabilities: None,
        }
    }

    #[test]
    fn legacy_single_workflow_resolves_to_default_entry() {
        let mut manifest = empty_manifest();
        manifest.workflow = Some("workflow.yaml".to_string());
        let resolved = manifest.resolved_workflows().unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "default");
        assert_eq!(resolved[0].path, "workflow.yaml");
        assert!(resolved[0].default);
    }

    #[test]
    fn no_workflow_field_defaults_to_workflow_yaml() {
        let manifest = empty_manifest();
        let resolved = manifest.resolved_workflows().unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].path, DEFAULT_WORKFLOW_FILE);
    }

    #[test]
    fn workflows_array_first_entry_is_implicit_default() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "plain".to_string(),
                path: "workflow.yaml".to_string(),
                description: None,
                default: false,
            },
            WorkflowEntry {
                name: "sharded".to_string(),
                path: "workflows/sharded.yaml".to_string(),
                description: Some("Sharded for big repos".to_string()),
                default: false,
            },
        ]);
        let resolved = manifest.resolved_workflows().unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved[0].default);
        assert!(!resolved[1].default);
        assert_eq!(manifest.default_workflow().unwrap().name, "plain");
    }

    #[test]
    fn explicit_default_overrides_first_entry() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "plain".to_string(),
                path: "workflow.yaml".to_string(),
                description: None,
                default: false,
            },
            WorkflowEntry {
                name: "sharded".to_string(),
                path: "workflows/sharded.yaml".to_string(),
                description: None,
                default: true,
            },
        ]);
        assert_eq!(manifest.default_workflow().unwrap().name, "sharded");
    }

    #[test]
    fn rejects_workflow_and_workflows_together() {
        let mut manifest = empty_manifest();
        manifest.workflow = Some("workflow.yaml".to_string());
        manifest.workflows = Some(vec![WorkflowEntry {
            name: "plain".to_string(),
            path: "workflow.yaml".to_string(),
            description: None,
            default: true,
        }]);
        assert!(manifest.validate_workflow_entries().is_err());
    }

    #[test]
    fn rejects_duplicate_workflow_names() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "plain".to_string(),
                path: "a.yaml".to_string(),
                description: None,
                default: false,
            },
            WorkflowEntry {
                name: "plain".to_string(),
                path: "b.yaml".to_string(),
                description: None,
                default: false,
            },
        ]);
        assert!(manifest.validate_workflow_entries().is_err());
    }

    #[test]
    fn rejects_duplicate_workflow_paths() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "a".to_string(),
                path: "workflow.yaml".to_string(),
                description: None,
                default: false,
            },
            WorkflowEntry {
                name: "b".to_string(),
                path: "workflow.yaml".to_string(),
                description: None,
                default: false,
            },
        ]);
        assert!(manifest.validate_workflow_entries().is_err());
    }

    #[test]
    fn rejects_invalid_workflow_name() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![WorkflowEntry {
            name: "Bad Name".to_string(),
            path: "workflow.yaml".to_string(),
            description: None,
            default: false,
        }]);
        assert!(manifest.validate_workflow_entries().is_err());
    }

    #[test]
    fn rejects_multiple_default_workflows() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "a".to_string(),
                path: "a.yaml".to_string(),
                description: None,
                default: true,
            },
            WorkflowEntry {
                name: "b".to_string(),
                path: "b.yaml".to_string(),
                description: None,
                default: true,
            },
        ]);
        assert!(manifest.validate_workflow_entries().is_err());
    }

    #[test]
    fn lenient_resolution_prefers_workflows_when_both_set() {
        let mut manifest = empty_manifest();
        manifest.workflow = Some("workflow.yaml".to_string());
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "Main".to_string(),
                path: "workflow.yaml".to_string(),
                description: None,
                default: true,
            },
            WorkflowEntry {
                name: "Sharded".to_string(),
                path: "workflow-sharded.yaml".to_string(),
                description: None,
                default: false,
            },
        ]);
        let resolved = manifest.resolved_workflows().expect("lenient");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "Main");
        assert!(resolved[0].default);
        assert!(!resolved[1].default);
    }

    #[test]
    fn lenient_resolution_collapses_multiple_defaults_to_first() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "Main".to_string(),
                path: "workflow.yaml".to_string(),
                description: None,
                default: true,
            },
            WorkflowEntry {
                name: "Sharded".to_string(),
                path: "workflow-sharded.yaml".to_string(),
                description: None,
                default: true,
            },
        ]);
        let resolved = manifest.resolved_workflows().unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved[0].default);
        assert!(!resolved[1].default);
        assert_eq!(manifest.default_workflow().unwrap().name, "Main");
    }

    #[test]
    fn lenient_resolution_accepts_mixed_case_names() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![WorkflowEntry {
            name: "Main".to_string(),
            path: "workflow.yaml".to_string(),
            description: None,
            default: true,
        }]);
        let resolved = manifest.resolved_workflows().unwrap();
        assert_eq!(resolved[0].name, "Main");
    }

    #[test]
    fn find_workflow_returns_named_entry() {
        let mut manifest = empty_manifest();
        manifest.workflows = Some(vec![
            WorkflowEntry {
                name: "plain".to_string(),
                path: "workflow.yaml".to_string(),
                description: None,
                default: true,
            },
            WorkflowEntry {
                name: "sharded".to_string(),
                path: "workflows/sharded.yaml".to_string(),
                description: None,
                default: false,
            },
        ]);
        let entry = manifest.find_workflow(Some("sharded")).unwrap();
        assert_eq!(entry.path, "workflows/sharded.yaml");
        assert!(manifest.find_workflow(Some("missing")).is_err());
        assert_eq!(manifest.find_workflow(None).unwrap().name, "plain");
    }
}

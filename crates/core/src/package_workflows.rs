//! Finding the workflow file of a codemod package. Every command and task
//! that runs a package's workflow finds it here.

use std::path::{Component, Path, PathBuf};

/// File names looked for, in this order, in a package that declares no
/// workflow.
pub const DEFAULT_WORKFLOW_FILES: &[&str] = &[
    "workflow.ts",
    "workflow.yaml",
    "workflow.yml",
    "workflow.json",
    "butterflow.yaml",
    "butterflow.json",
];

/// A workflow `codemod.yaml` declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredWorkflow {
    pub name: String,
    pub path: PathBuf,
    pub default: bool,
}

/// The workflows `codemod.yaml` in `dir` declares, with `workflows:` (a list
/// of `{ name, path, default }`) or `workflow:` (one path, named `default`).
/// Empty when it declares none, or there is no `codemod.yaml`.
pub fn declared_workflows(dir: &Path) -> Result<Vec<DeclaredWorkflow>, String> {
    let manifest_path = dir.join("codemod.yaml");
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", manifest_path.display())),
    };
    let manifest: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|error| format!("cannot parse {}: {error}", manifest_path.display()))?;
    let inside = |path: &str| -> Result<PathBuf, String> {
        let relative = Path::new(path.trim());
        let stays = relative
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
        if stays && !path.trim().is_empty() {
            Ok(dir.join(relative))
        } else {
            Err(format!(
                "{}: workflow path '{path}' must be relative and stay inside the package",
                manifest_path.display()
            ))
        }
    };

    let listed = manifest
        .get("workflows")
        .and_then(|workflows| workflows.as_sequence())
        .filter(|entries| !entries.is_empty());
    if let Some(entries) = listed {
        let mut workflows = Vec::new();
        for entry in entries {
            let text = |key: &str| entry.get(key).and_then(|value| value.as_str());
            let (Some(name), Some(path)) = (text("name"), text("path")) else {
                return Err(format!(
                    "{}: every entry of `workflows` needs a name and a path",
                    manifest_path.display()
                ));
            };
            workflows.push(DeclaredWorkflow {
                name: name.to_string(),
                path: inside(path)?,
                default: entry
                    .get("default")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
            });
        }
        // Exactly one default: the first marked so, else the first.
        let default = workflows
            .iter()
            .position(|workflow| workflow.default)
            .unwrap_or(0);
        for (index, workflow) in workflows.iter_mut().enumerate() {
            workflow.default = index == default;
        }
        return Ok(workflows);
    }
    match manifest.get("workflow").and_then(|value| value.as_str()) {
        Some(path) if !path.trim().is_empty() => Ok(vec![DeclaredWorkflow {
            name: "default".to_string(),
            path: inside(path)?,
            default: true,
        }]),
        _ => Ok(Vec::new()),
    }
}

/// The workflow file to run for `source`: the file itself, or for a package
/// directory the workflow named `name`, else the one its `codemod.yaml`
/// declares as default, else the first of [`DEFAULT_WORKFLOW_FILES`] in it.
pub fn locate_workflow(source: &Path, name: Option<&str>) -> Result<PathBuf, String> {
    let absolute = |path: &Path| {
        path.canonicalize()
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    if source.is_file() {
        if name.is_some() {
            return Err(format!(
                "cannot select a named workflow when the source is a workflow file: {}",
                source.display()
            ));
        }
        return absolute(source);
    }
    if !source.is_dir() {
        return Err(format!(
            "workflow source does not exist: {}",
            source.display()
        ));
    }
    let dir = absolute(source)?;
    let declared = declared_workflows(&dir)?;
    let chosen = match name {
        Some(name) => Some(
            declared
                .iter()
                .find(|workflow| workflow.name == name)
                .ok_or_else(|| {
                    if declared.is_empty() {
                        format!(
                            "cannot select a named workflow: codemod.yaml in {} declares none",
                            dir.display()
                        )
                    } else {
                        let available: Vec<&str> = declared
                            .iter()
                            .map(|workflow| workflow.name.as_str())
                            .collect();
                        format!(
                            "workflow `{name}` not found in codemod.yaml; available: {}",
                            available.join(", ")
                        )
                    }
                })?,
        ),
        None => declared.iter().find(|workflow| workflow.default),
    };
    if let Some(workflow) = chosen {
        if workflow.path.is_file() {
            return Ok(workflow.path.clone());
        }
        return Err(format!(
            "workflow `{}` declared in codemod.yaml is missing: {}",
            workflow.name,
            workflow.path.display()
        ));
    }
    DEFAULT_WORKFLOW_FILES
        .iter()
        .map(|file| dir.join(file))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "no workflow file (workflow.ts or workflow.yaml) found in {}",
                dir.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        dir
    }

    fn name_of(path: PathBuf) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn a_package_without_a_declaration_uses_the_first_default_file() {
        let dir = package(&[("workflow.yaml", ""), ("workflow.ts", "")]);
        assert_eq!(
            name_of(locate_workflow(dir.path(), None).unwrap()),
            "workflow.ts"
        );
        // A manifest that names no workflow changes nothing.
        std::fs::write(dir.path().join("codemod.yaml"), "name: demo\n").unwrap();
        assert_eq!(
            name_of(locate_workflow(dir.path(), None).unwrap()),
            "workflow.ts"
        );
        let legacy = package(&[("butterflow.yaml", "")]);
        assert_eq!(
            name_of(locate_workflow(legacy.path(), None).unwrap()),
            "butterflow.yaml"
        );
    }

    #[test]
    fn the_manifest_decides_when_it_declares_workflows() {
        let dir = package(&[
            ("workflow.ts", ""),
            ("main.yaml", ""),
            ("sharded.yaml", ""),
            (
                "codemod.yaml",
                "workflows:\n  - name: main\n    path: main.yaml\n  - name: sharded\n    path: ./sharded.yaml\n    default: true\n",
            ),
        ]);
        assert_eq!(
            name_of(locate_workflow(dir.path(), None).unwrap()),
            "sharded.yaml"
        );
        assert_eq!(
            name_of(locate_workflow(dir.path(), Some("main")).unwrap()),
            "main.yaml"
        );
        let error = locate_workflow(dir.path(), Some("other")).unwrap_err();
        assert!(error.contains("available: main, sharded"), "{error}");
    }

    #[test]
    fn a_declared_workflow_that_is_missing_is_an_error_not_a_fallback() {
        let dir = package(&[
            ("workflow.ts", ""),
            ("codemod.yaml", "workflow: gone.yaml\n"),
        ]);
        let error = locate_workflow(dir.path(), None).unwrap_err();
        assert!(error.contains("is missing"), "{error}");
        let escape = package(&[("codemod.yaml", "workflow: ../elsewhere.yaml\n")]);
        let error = locate_workflow(escape.path(), None).unwrap_err();
        assert!(error.contains("stay inside the package"), "{error}");
    }

    #[test]
    fn a_file_is_its_own_workflow() {
        let dir = package(&[("custom.ts", "")]);
        let file = dir.path().join("custom.ts");
        assert_eq!(name_of(locate_workflow(&file, None).unwrap()), "custom.ts");
        assert!(locate_workflow(&file, Some("main")).is_err());
    }
}

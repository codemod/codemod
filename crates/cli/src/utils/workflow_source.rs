//! Finding the workflow file a command should run.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Resolves a workflow source (a file, or a directory) to the workflow file
/// and its directory. When `source` is a directory and
/// contains a `codemod.yaml`, honors that manifest's workflow declaration(s)
/// and lets the caller pick a workflow by name.
pub fn resolve_workflow_source_with_name(
    source: &str,
    workflow_name: Option<&str>,
) -> Result<(PathBuf, PathBuf)> {
    let path = PathBuf::from(source);

    if !path.exists() {
        // TODO: Add registry lookup logic here in the future
        return Err(anyhow::anyhow!(
            "Workflow source path does not exist: {}",
            source
        ));
    }

    if path.is_dir() {
        let bundle_path = path.canonicalize().context(format!(
            "Failed to get absolute path for bundle directory: {}",
            path.display()
        ))?;

        let manifest_path = bundle_path.join("codemod.yaml");
        let manifest = if manifest_path.is_file() {
            let manifest_content = std::fs::read_to_string(&manifest_path).context(format!(
                "Failed to read manifest {}",
                manifest_path.display()
            ))?;
            match serde_yaml::from_str::<crate::utils::manifest::CodemodManifest>(&manifest_content)
            {
                Ok(m) => Some(m),
                Err(error) => {
                    if workflow_name.is_some() {
                        return Err(anyhow::anyhow!(
                            "Failed to parse manifest {}: {error}",
                            manifest_path.display()
                        ));
                    }
                    log::warn!(
                        "Ignoring unparseable codemod.yaml at {}: {error}",
                        manifest_path.display()
                    );
                    None
                }
            }
        } else {
            None
        };

        if let Some(manifest) = manifest {
            match manifest.find_workflow(workflow_name) {
                Ok(entry) => {
                    let workflow_file_path = bundle_path.join(&entry.path);
                    if workflow_file_path.is_file() {
                        return Ok((workflow_file_path, bundle_path));
                    }
                    if workflow_name.is_some() {
                        return Err(anyhow::anyhow!(
                            "Workflow `{}` declared in codemod.yaml is missing at {}",
                            entry.name,
                            workflow_file_path.display()
                        ));
                    }
                    log::warn!(
                        "Workflow `{}` declared in codemod.yaml is missing at {}; falling back to default workflow discovery",
                        entry.name,
                        workflow_file_path.display()
                    );
                }
                Err(error) => {
                    if workflow_name.is_some() {
                        return Err(error);
                    }
                    log::warn!("{error}");
                }
            }
        } else if workflow_name.is_some() {
            return Err(anyhow::anyhow!(
                "Cannot select a named workflow without a codemod.yaml in {}",
                bundle_path.display()
            ));
        }

        // Look for default workflow files within the directory
        let default_files = [
            "workflow.ts",
            "workflow.yaml",
            "butterflow.yaml",
            "workflow.json",
            "butterflow.json",
        ];
        let mut workflow_file_path = None;

        for file_name in default_files.iter() {
            let potential_path = bundle_path.join(file_name);
            if potential_path.is_file() {
                workflow_file_path = Some(potential_path);
                break;
            }
        }

        match workflow_file_path {
            Some(file) => Ok((file, bundle_path)),
            None => Err(anyhow::anyhow!(
                "No workflow file (workflow.ts or workflow.yaml) found in directory: {}",
                bundle_path.display()
            )),
        }
    } else if path.is_file() {
        if workflow_name.is_some() {
            return Err(anyhow::anyhow!(
                "Cannot select a named workflow when the source points at a workflow file: {}",
                source
            ));
        }
        let workflow_file_path = path.canonicalize().context(format!(
            "Failed to get absolute path for workflow file: {}",
            path.display()
        ))?;
        let bundle_path = workflow_file_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Could not get parent directory for workflow file"))?
            .to_path_buf();
        Ok((workflow_file_path, bundle_path))
    } else {
        Err(anyhow::anyhow!(
            "Workflow source path is neither a file nor a directory: {}",
            source
        ))
    }
}

//! The native addon behind the `codemod` npm package's Node.js and Bun host.
//!
//! Node runs the workflow body, function steps and `forEachFile` callbacks
//! itself, with the shared orchestrator; this addon gives it what the engine
//! owns ([`run::Run`]: the run store, host tasks, file selection and commits)
//! and the `codemod:ast-grep` API ([`ast`]).
//!
//! There is no capability sandbox here: code in Node has whatever Node gives
//! it. Host tasks (`jssg`, `codemod`, ...) still run in the engine, sandboxed
//! as usual.

pub mod ast;
pub mod run;

/// The orchestrator source the engine runs in QuickJS, so the npm package
/// can check it ships the same one.
#[napi_derive::napi]
pub fn orchestrator_source() -> String {
    codemod_workflow::ORCHESTRATOR_JS.to_string()
}

/// The workflow file a source names: the file itself, or for a package
/// directory the workflow `codemod.yaml` declares (by `name`, or its
/// default), falling back to `workflow.ts`.
#[napi_derive::napi]
pub fn resolve_workflow(source: String, name: Option<String>) -> napi::Result<String> {
    resolve::workflow(std::path::Path::new(&source), name.as_deref())
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(napi::Error::from_reason)
}

mod resolve {
    use std::path::{Path, PathBuf};

    use serde::Deserialize;

    #[derive(Deserialize, Default)]
    struct Manifest {
        workflow: Option<String>,
        #[serde(default)]
        workflows: Vec<Entry>,
    }

    #[derive(Deserialize)]
    struct Entry {
        name: String,
        path: String,
        #[serde(default)]
        default: bool,
    }

    pub fn workflow(source: &Path, name: Option<&str>) -> Result<PathBuf, String> {
        if source.is_file() {
            if name.is_some() {
                return Err(format!(
                    "cannot select a named workflow when the source is a workflow file: {}",
                    source.display()
                ));
            }
            return source
                .canonicalize()
                .map_err(|e| format!("{}: {e}", source.display()));
        }
        if !source.is_dir() {
            return Err(format!(
                "workflow source does not exist: {}",
                source.display()
            ));
        }
        let dir = source
            .canonicalize()
            .map_err(|e| format!("{}: {e}", source.display()))?;
        let manifest_path = dir.join("codemod.yaml");
        let manifest: Manifest = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => serde_yaml::from_str(&text)
                .map_err(|e| format!("{}: {e}", manifest_path.display()))?,
            Err(_) if name.is_some() => {
                return Err(format!(
                    "cannot select a named workflow without a codemod.yaml in {}",
                    dir.display()
                ))
            }
            Err(_) => Manifest::default(),
        };
        let declared = if !manifest.workflows.is_empty() {
            let entry = match name {
                Some(name) => manifest.workflows.iter().find(|entry| entry.name == name),
                None => manifest
                    .workflows
                    .iter()
                    .find(|entry| entry.default)
                    .or(manifest.workflows.first()),
            };
            match (entry, name) {
                (Some(entry), _) => Some(entry.path.clone()),
                (None, Some(name)) => {
                    let available: Vec<&str> = manifest
                        .workflows
                        .iter()
                        .map(|entry| entry.name.as_str())
                        .collect();
                    return Err(format!(
                        "workflow `{name}` not found in codemod.yaml; available: {}",
                        available.join(", ")
                    ));
                }
                (None, None) => None,
            }
        } else {
            manifest.workflow.filter(|path| !path.trim().is_empty())
        };
        if let Some(path) = declared {
            let path = dir.join(path.trim());
            if path.is_file() {
                return Ok(path);
            }
            return Err(format!(
                "the workflow codemod.yaml declares is missing: {}",
                path.display()
            ));
        }
        for file in ["workflow.ts", "workflow.yaml", "workflow.json"] {
            let path = dir.join(file);
            if path.is_file() {
                return Ok(path);
            }
        }
        Err(format!(
            "no workflow file (workflow.ts or workflow.yaml) in {}",
            dir.display()
        ))
    }
}

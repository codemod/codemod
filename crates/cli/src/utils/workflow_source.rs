//! Finding the workflow file a command should run.

use std::path::PathBuf;

use anyhow::Result;
use butterflow_core::package_workflows::locate_workflow;

/// Resolves a workflow source (a file, or a package directory) to the
/// workflow file and its directory; see [`locate_workflow`].
pub fn resolve_workflow_source_with_name(
    source: &str,
    workflow_name: Option<&str>,
) -> Result<(PathBuf, PathBuf)> {
    let workflow = locate_workflow(std::path::Path::new(source), workflow_name)
        .map_err(|error| anyhow::anyhow!(error))?;
    let bundle = workflow
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not get parent directory for workflow file"))?
        .to_path_buf();
    Ok((workflow, bundle))
}

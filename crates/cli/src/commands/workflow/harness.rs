//! `codemod workflow harness`: hidden. Runs one workflow test for the
//! `codemod/testing` helpers in the npm package. Reads a JSON spec on stdin
//! and writes the result as one JSON line on stdout.

use std::io::Read as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_workflow::TestPlan;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::engine::create_registry_client;
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::utils::workflow_source::resolve_workflow_source_with_name;
use crate::workflow_test::{run_case, CaseRequest};

#[derive(Args, Debug)]
pub struct Command {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Spec {
    /// A workflow file or a package directory.
    workflow: String,
    #[serde(default)]
    workflow_name: Option<String>,
    /// The directory the run changes (the helpers make a copy first).
    target: PathBuf,
    #[serde(default)]
    params: Map<String, Value>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    given: Map<String, Value>,
    #[serde(default = "yes")]
    approve: bool,
    /// Capabilities granted on top of those codemod.yaml declares.
    #[serde(default)]
    capabilities: Vec<String>,
}

fn yes() -> bool {
    true
}

pub async fn handler(_args: &Command) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("cannot read the test spec from stdin")?;
    let spec: Spec = serde_json::from_str(&input).context("invalid test spec")?;

    let (workflow, workflow_dir) =
        resolve_workflow_source_with_name(&spec.workflow, spec.workflow_name.as_deref())?;
    let mut capabilities = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: false,
            allow_fetch: false,
            allow_child_process: false,
        },
        None,
        Some(workflow_dir),
    );
    for name in &spec.capabilities {
        let module = name
            .parse::<LlrtSupportedModules>()
            .map_err(|_| anyhow::anyhow!("unknown capability '{name}'"))?;
        capabilities.insert(module);
    }

    let run = run_case(CaseRequest {
        workflow,
        target: spec.target,
        params: spec.params,
        plan: TestPlan {
            until: spec.until,
            given: spec.given,
        },
        approve: spec.approve,
        capabilities,
        registry: create_registry_client(None).ok(),
    })
    .await?;
    println!("{}", run.to_json());
    Ok(())
}

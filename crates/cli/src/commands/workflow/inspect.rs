use anyhow::{anyhow, Context, Result};
use clap::Args;
use serde_json::{json, Value};

use super::status::ListFormat;
use crate::utils::resolve_capabilities::{resolve_capabilities, ResolveCapabilitiesArgs};
use crate::utils::workflow_source::resolve_workflow_source_with_name;

#[derive(Args, Debug)]
pub struct Command {
    /// Path to a workflow file (workflow.ts or workflow.yaml) or a directory
    #[arg(short, long, value_name = "PATH")]
    workflow: String,

    /// Name of the workflow when codemod.yaml defines several
    #[arg(long = "workflow-name", value_name = "NAME")]
    workflow_name: Option<String>,

    /// Output format: "text" (default), or "json" for programs (a setup form)
    #[arg(long, value_enum, default_value_t = ListFormat::Text)]
    format: ListFormat,
}

/// Describe a workflow without running it: the params it declares, the
/// modules it imports, the declared steps that reach outside the target
/// (`http`, `shell`, `agent`, ...), and its steps. Capabilities its package
/// asks for are listed as `requestedCapabilities`: callbacks no longer get
/// them, so `capabilities` is always empty.
pub async fn handler(args: &Command) -> Result<()> {
    let (path, dir) =
        resolve_workflow_source_with_name(&args.workflow, args.workflow_name.as_deref())?;
    let typescript = codemod_workflow::is_typescript(&path);
    let source = if typescript {
        std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?
    } else {
        codemod_workflow::compat::translate_file(&path)
            .map_err(|error| anyhow!("{error}"))?
            .source
    };
    let file = path.display().to_string();
    let inspection =
        codemod_workflow::inspect(&file, &source).map_err(|error| anyhow!("{error}"))?;
    let params = codemod_workflow::params::declared_params(&source);
    let modules = codemod_workflow::params::imported_modules(&source);
    let mut requested: Vec<String> = resolve_capabilities(
        ResolveCapabilitiesArgs {
            allow_fs: false,
            allow_fetch: false,
            allow_child_process: false,
        },
        None,
        Some(dir),
    )
    .into_iter()
    .map(|capability| capability.as_str().to_string())
    .collect();
    requested.sort();
    let calls = |names: &[&str]| names.iter().any(|name| inspection.calls(name));
    let described = json!({
        "workflow": path,
        "language": if typescript { "typescript" } else { "yaml" },
        "params": params.iter().map(|param| json!({
            "name": param.name,
            "type": param.type_text,
            "optional": param.optional,
            "description": param.description,
            "default": param.default,
        })).collect::<Vec<_>>(),
        "modules": modules,
        // Callbacks get no capabilities: kept for programs that read it.
        "capabilities": [],
        "requestedCapabilities": requested,
        "uses": {
            "http": calls(&["http"]),
            "approval": calls(&["approval"]),
            "agent": calls(&["agent"]),
            "shell": calls(&["shell"]),
            "git": calls(&["branch", "commit", "pullRequest"]),
            "codemod": calls(&["codemod"]),
            "install_skill": calls(&["installSkill"]),
            "wait": calls(&["sleep", "waitForEvent"]),
            "assess": modules.iter().any(|module| matches!(module.as_str(), "codemod/assess" | "codemod:assess")),
        },
        "steps": inspection.steps.iter().map(|step| json!({
            "label": step.label,
            "kind": step.kind,
        })).collect::<Vec<_>>(),
    });
    if args.format == ListFormat::Json {
        println!("{described}");
        return Ok(());
    }
    print_text(&described);
    Ok(())
}

fn print_text(described: &Value) {
    println!(
        "Workflow:  {}",
        described["workflow"].as_str().unwrap_or_default()
    );
    let params = described["params"].as_array().cloned().unwrap_or_default();
    if params.is_empty() {
        println!("Params:    none declared");
    } else {
        println!("Params:");
        for param in params {
            let mut line = format!(
                "  {}{}: {}",
                param["name"].as_str().unwrap_or_default(),
                if param["optional"] == true { "?" } else { "" },
                param["type"].as_str().unwrap_or_default()
            );
            if !param["default"].is_null() {
                line.push_str(&format!(" = {}", param["default"]));
            }
            if let Some(description) = param["description"].as_str() {
                line.push_str(&format!("  ({description})"));
            }
            println!("{line}");
        }
    }
    let uses = &described["uses"];
    let reaches: Vec<&str> = [
        ("http", "network (http)"),
        ("shell", "commands (shell)"),
        ("agent", "coding agents (agent)"),
        ("codemod", "other codemods (codemod)"),
        ("git", "git (branch, commit, pullRequest)"),
    ]
    .into_iter()
    .filter(|(key, _)| uses[*key] == true)
    .map(|(_, what)| what)
    .collect();
    if !reaches.is_empty() {
        println!("Reaches:   {}", reaches.join(", "));
    }
    let requested: Vec<&str> = described["requestedCapabilities"]
        .as_array()
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !requested.is_empty() {
        println!(
            "Asks for:  {} (ignored: callbacks no longer get capabilities; use http() and shell() steps)",
            requested.join(", ")
        );
    }
    let steps = described["steps"].as_array().cloned().unwrap_or_default();
    if !steps.is_empty() {
        println!("Steps:");
        for step in steps {
            println!(
                "  {} ({})",
                step["label"].as_str().unwrap_or("(no label)"),
                step["kind"].as_str().unwrap_or("?")
            );
        }
    }
}

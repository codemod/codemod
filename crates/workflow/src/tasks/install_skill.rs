//! `installSkill({ package, path, harness, scope, force })`.

use butterflow_core::config::InstallSkillExecutionRequest;
use butterflow_core::structured_log::OutputFormat;
use butterflow_models::step::UseInstallSkill;
use serde_json::{json, Value};

use super::{block_on, child_env, TaskContext, TaskError, TaskOutcome, TaskResult};

pub(crate) fn run(task: &TaskContext<'_>, content: Value) -> TaskResult {
    let run = task.run;
    let install_skill: UseInstallSkill = serde_json::from_value(content)
        .map_err(|e| TaskError::Failed(format!("invalid installSkill options: {e}")))?;
    if run.dry_run {
        task.log("skipped: skills are not installed in a dry run");
        return Ok(TaskOutcome::new(json!({ "skipped": true })));
    }
    let Some(executor) = run.services.install_skill.clone() else {
        return Err(TaskError::Failed(
            "installing skills is not available in this environment".into(),
        ));
    };
    let output = block_on(executor.execute(InstallSkillExecutionRequest {
        install_skill,
        no_interactive: !run.interaction.is_interactive(),
        quiet: true,
        bundle_path: Some(run.bundle_dir.clone()),
        target_path: run.target.clone(),
        env: child_env(task),
        output_format: OutputFormat::Text,
        selection_prompt_callback: None,
    }))
    .map_err(|e| TaskError::Failed(e.to_string()))?;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        task.log(line.to_string());
    }
    Ok(TaskOutcome::new(json!({ "output": output })))
}

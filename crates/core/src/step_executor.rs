use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
};

use butterflow_models::step::StepAction;
use butterflow_models::{evaluate_condition, Node, Result, Task, TaskExpressionContext, Workflow};
use butterflow_runners::Runner;
use codemod_llrt_capabilities::types::LlrtSupportedModules;

use crate::{
    config::{DeferredInteractionError, InstallSkillExecutionRequest},
    engine::{
        auto_meta_files_include, execute_install_skill_in_isolated_runtime, log_step_output,
        resolve_optional_glob_list, CapabilitiesData, CodemodDependency, Engine,
    },
    slog,
    structured_log::StructuredLogger,
    Error,
};

pub(crate) struct StepExecutionRequest<'a> {
    pub runner: &'a dyn Runner,
    pub action: &'a StepAction,
    pub step_name: &'a str,
    pub step_env: &'a Option<HashMap<String, String>>,
    pub step_id: &'a Option<String>,
    pub report_step_name: Option<&'a str>,
    pub report_step_id: Option<&'a String>,
    pub node: &'a Node,
    pub task: &'a Task,
    pub params: &'a HashMap<String, serde_json::Value>,
    pub state: &'a HashMap<String, serde_json::Value>,
    pub workflow: &'a Workflow,
    pub bundle_path: &'a Option<PathBuf>,
    pub dependency_chain: &'a [CodemodDependency],
    pub capabilities: &'a Option<HashSet<LlrtSupportedModules>>,
    pub task_expr_ctx: Option<&'a TaskExpressionContext>,
    pub progress_task_id: Option<&'a str>,
    pub logger: &'a StructuredLogger,
}

pub(crate) struct StepExecutor<'a> {
    engine: &'a Engine,
}

impl<'a> StepExecutor<'a> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self { engine }
    }

    pub(crate) fn execute<'b>(
        &'b self,
        request: StepExecutionRequest<'b>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'b>> {
        Box::pin(async move {
            match request.action {
                StepAction::RunScript(run) => {
                    if self.engine.workflow_run_config().execution.dry_run {
                        return Ok(());
                    }

                    self.engine
                        .execute_run_script_step(
                            request.runner,
                            run,
                            request.step_name,
                            request.step_env,
                            request.step_id,
                            request.node,
                            request.task,
                            request.params,
                            request.state,
                            request.bundle_path,
                            request.logger,
                        )
                        .await
                }
                StepAction::UseTemplate(template_use) => {
                    let template = request
                        .workflow
                        .templates
                        .iter()
                        .find(|t| t.id == template_use.template)
                        .ok_or_else(|| {
                            Error::Template(format!(
                                "Template not found: {}",
                                template_use.template
                            ))
                        })?;

                    let mut combined_params = request.params.clone();
                    combined_params.extend(template_use.inputs.clone());

                    for template_step in &template.steps {
                        if let Some(condition) = &template_step.condition {
                            let should_execute = evaluate_condition(
                                condition,
                                &combined_params,
                                request.state,
                                request.task.matrix_values.as_ref(),
                                None,
                                request.task_expr_ctx,
                            )?;

                            if !should_execute {
                                slog!(
                                    request.logger,
                                    info,
                                    "Skipping template step '{}' - condition not met: {}",
                                    template_step.name,
                                    condition
                                );
                                continue;
                            }
                        }

                        self.execute(StepExecutionRequest {
                            runner: request.runner,
                            action: &template_step.action,
                            step_name: &template_step.name,
                            step_env: &template_step.env,
                            step_id: &template_step.id,
                            report_step_name: request.report_step_name,
                            report_step_id: request.report_step_id,
                            node: request.node,
                            task: request.task,
                            params: &combined_params,
                            state: request.state,
                            workflow: request.workflow,
                            bundle_path: request.bundle_path,
                            dependency_chain: request.dependency_chain,
                            capabilities: request.capabilities,
                            task_expr_ctx: request.task_expr_ctx,
                            progress_task_id: request.progress_task_id,
                            logger: request.logger,
                        })
                        .await?;
                    }
                    Ok(())
                }
                StepAction::AstGrep(ast_grep) => {
                    let mut resolved_ast_grep = ast_grep.clone();
                    resolved_ast_grep.include = resolve_optional_glob_list(
                        &ast_grep.include,
                        request.params,
                        request.state,
                        request.task.matrix_values.as_ref(),
                        request.task_expr_ctx,
                    )?
                    .or_else(|| {
                        auto_meta_files_include(request.state, request.task.matrix_values.as_ref())
                    });
                    resolved_ast_grep.exclude = resolve_optional_glob_list(
                        &ast_grep.exclude,
                        request.params,
                        request.state,
                        request.task.matrix_values.as_ref(),
                        request.task_expr_ctx,
                    )?;
                    self.engine
                        .execute_ast_grep_step(
                            request
                                .progress_task_id
                                .unwrap_or(&request.node.id)
                                .to_string(),
                            &resolved_ast_grep,
                            request.logger,
                        )
                        .await
                }
                StepAction::JSAstGrep(js_ast_grep) => {
                    let progress_task_id = request
                        .progress_task_id
                        .map(str::to_string)
                        .unwrap_or_else(|| request.task.id.to_string());
                    self.engine
                        .execute_js_ast_grep_step(
                            request.task.id.to_string(),
                            Some(progress_task_id),
                            request.step_id.clone().unwrap_or_default(),
                            request.step_name.to_string(),
                            request.report_step_id.cloned(),
                            request.report_step_name.map(str::to_string),
                            js_ast_grep,
                            Some(request.params.clone()),
                            request.task.matrix_values.clone(),
                            &CapabilitiesData {
                                capabilities: request
                                    .capabilities
                                    .as_ref()
                                    .map(|v| v.clone().into_iter().collect()),
                                capabilities_security_callback: self
                                    .engine
                                    .workflow_run_config()
                                    .execution
                                    .capabilities_security_callback
                                    .clone(),
                            },
                            request.bundle_path,
                            Some(request.task.workflow_run_id),
                            Some(request.state),
                            request.logger,
                            None,
                            None,
                            request.task_expr_ctx,
                        )
                        .await
                }
                StepAction::Codemod(codemod) => {
                    self.engine
                        .execute_codemod_step(
                            codemod,
                            request.report_step_name.unwrap_or(request.step_name),
                            request.report_step_id.or(request.step_id.as_ref()),
                            request.step_env,
                            request.node,
                            request.task,
                            request.params,
                            request.state,
                            request.bundle_path,
                            request.dependency_chain,
                            request.capabilities,
                            request.logger,
                        )
                        .await
                }
                StepAction::AI(ai_config) => {
                    if self.engine.workflow_run_config().execution.dry_run {
                        slog!(request.logger, info, "Skipping AI step in dry-run mode");
                        return Ok(());
                    }

                    self.engine
                        .execute_ai_step(
                            ai_config,
                            request.step_env,
                            request.node,
                            request.task,
                            request.params,
                            request.state,
                            request.logger,
                        )
                        .await
                }
                StepAction::Shard(shard_config) => {
                    if self.engine.workflow_run_config().execution.skip_shard_steps {
                        slog!(
                            request.logger,
                            info,
                            "Skipping shard step in dry-run preview mode"
                        );
                        return Ok(());
                    }

                    self.engine
                        .execute_shard_step(
                            shard_config,
                            request.task,
                            request.params,
                            request.state,
                            request.task_expr_ctx,
                            request.logger,
                        )
                        .await
                }
                StepAction::InstallSkill(install_skill) => {
                    if self
                        .engine
                        .workflow_run_config()
                        .skill_install
                        .skip_install_skill_steps
                    {
                        if self.engine.workflow_run_config().interaction.no_interactive {
                            slog!(
                                request.logger,
                                info,
                                "install-skill step skipped in non-interactive mode by default. Re-run with --install-skill to execute this step: package={}",
                                install_skill.package
                            );
                        } else {
                            slog!(
                                request.logger,
                                info,
                                "Skipping install-skill step in this run mode: package={}",
                                install_skill.package
                            );
                        }
                        return Ok(());
                    }

                    if self.engine.workflow_run_config().execution.dry_run {
                        slog!(
                            request.logger,
                            warn,
                            "Skipping install-skill step in dry-run mode: package={}",
                            install_skill.package
                        );
                        return Ok(());
                    }

                    let Some(install_skill_executor) = self
                        .engine
                        .workflow_run_config()
                        .skill_install
                        .install_skill_executor
                        .as_ref()
                    else {
                        return Err(Error::Runtime(
                        "install-skill step requested but no install-skill executor is configured"
                            .to_string(),
                    ));
                    };
                    let install_skill_executor = Arc::clone(install_skill_executor);

                    let prepared = self.engine.prepare_step_execution(
                        request.step_env,
                        request.node,
                        request.task,
                        request.state,
                        request.bundle_path,
                    )?;
                    let runtime_request = InstallSkillExecutionRequest {
                        install_skill: install_skill.clone(),
                        no_interactive: self
                            .engine
                            .workflow_run_config()
                            .interaction
                            .no_interactive,
                        quiet: self.engine.workflow_run_config().output.quiet,
                        bundle_path: request.bundle_path.clone(),
                        target_path: self
                            .engine
                            .workflow_run_config()
                            .execution
                            .target_path
                            .clone(),
                        env: prepared.env.clone(),
                        output_format: self.engine.workflow_run_config().output.output_format,
                        selection_prompt_callback: self
                            .engine
                            .workflow_run_config()
                            .interaction
                            .selection_prompt_callback
                            .clone(),
                    };
                    let output = execute_install_skill_in_isolated_runtime(
                        install_skill_executor,
                        runtime_request,
                    )
                    .await
                    .map_err(|error| {
                        if let Some(deferred) = error.downcast_ref::<DeferredInteractionError>() {
                            Error::Deferred(deferred.message().to_string())
                        } else {
                            Error::Runtime(format!("Failed to execute install-skill step: {error}"))
                        }
                    });

                    let output = match output {
                        Ok(output) => output,
                        Err(Error::Deferred(message)) => return Err(Error::Deferred(message)),
                        Err(error) => return Err(error),
                    };

                    for line in output
                        .lines()
                        .map(str::trim_end)
                        .filter(|line| !line.is_empty())
                    {
                        let _ = self
                            .engine
                            .append_task_log(request.task.id, line.to_string())
                            .await;
                    }
                    log_step_output(request.logger, &output);
                    self.engine
                        .finalize_step_execution(request.task, output, prepared)
                        .await
                }
            }
        })
    }
}

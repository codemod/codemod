use std::{collections::HashMap, path::PathBuf, sync::Arc};

use butterflow_models::{Error, Node, Result, Task, TaskExpressionContext, WorkflowRun};
use uuid::Uuid;

use crate::{
    config::{ManagedGitWorktree, PullRequestCreationRequest},
    engine::{
        pull_request_metadata_log_line, resolve_workflow_run_params, should_manage_git_for_node,
        Engine, ResolvedPullRequestConfig,
    },
    git_ops, slog,
    structured_log::StepContext,
};

pub(crate) struct ManagedGitService<'a> {
    engine: &'a Engine,
}

pub(crate) type WorktreeCleanup = Arc<std::sync::Mutex<Option<(PathBuf, PathBuf)>>>;

enum PullRequestOutcome {
    Deferred,
    Created(Option<String>),
}

impl<'a> ManagedGitService<'a> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self { engine }
    }

    pub(crate) fn resolve_pull_request_config(
        &self,
        task: &Task,
        node: &Node,
        params: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<ResolvedPullRequestConfig>> {
        if !should_manage_git_for_node(
            node,
            self.engine
                .workflow_run_config()
                .managed_git
                .enable_managed_git,
        ) {
            return Ok(None);
        }

        let task_expr_ctx = git_ops::build_task_expression_context(&task.id.to_string());
        let configured_branch = node.branch_name.as_ref().map(|tmpl| {
            butterflow_models::resolve_string_with_expression(
                tmpl,
                params,
                &HashMap::new(),
                task.matrix_values.as_ref(),
                None,
                Some(&task_expr_ctx),
            )
            .unwrap_or_else(|_| format!("codemod-{}", task_expr_ctx.signature))
        });
        let branch =
            git_ops::resolve_branch_name(configured_branch.as_deref(), &task_expr_ctx.signature);

        let (title, body, draft, base) = if let Some(pr_config) = &node.pull_request {
            let title = butterflow_models::resolve_string_with_expression(
                &pr_config.title,
                params,
                &HashMap::new(),
                task.matrix_values.as_ref(),
                None,
                Some(&task_expr_ctx),
            )
            .unwrap_or_else(|_| pr_config.title.clone());

            let body = pr_config.body.as_ref().map(|b| {
                butterflow_models::resolve_string_with_expression(
                    b,
                    params,
                    &HashMap::new(),
                    task.matrix_values.as_ref(),
                    None,
                    Some(&task_expr_ctx),
                )
                .unwrap_or_else(|_| b.clone())
            });

            (
                title,
                body,
                pr_config.draft.unwrap_or(false),
                pr_config.base.clone(),
            )
        } else {
            (node.name.clone(), None, false, None)
        };

        Ok(Some(ResolvedPullRequestConfig {
            title,
            body,
            draft,
            base,
            branch,
        }))
    }

    pub(crate) async fn create_pull_request_for_task(
        &self,
        task_id: Uuid,
    ) -> Result<Option<String>> {
        let task = self
            .engine
            .state_adapter()
            .lock()
            .await
            .get_task(task_id)
            .await?;
        let workflow_run = self
            .engine
            .state_adapter()
            .lock()
            .await
            .get_workflow_run(task.workflow_run_id)
            .await?;
        let node = workflow_run
            .workflow
            .nodes
            .iter()
            .find(|node| node.id == task.node_id)
            .ok_or_else(|| Error::Runtime(format!("Node '{}' not found for task", task.node_id)))?;
        let resolved_params = resolve_workflow_run_params(&workflow_run);

        let pr = self
            .resolve_pull_request_config(&task, node, &resolved_params)?
            .ok_or_else(|| {
                Error::Runtime("Task is not eligible for pull request creation".to_string())
            })?;

        let _ = self
            .engine
            .append_task_log(task_id, pull_request_metadata_log_line(&pr))
            .await;
        let _ = self
            .engine
            .append_task_log(task_id, "Publishing branch and creating pull request")
            .await;

        let pr_url = match async {
            git_ops::push_branch(
                &pr.branch,
                &self.engine.workflow_run_config().execution.target_path,
            )
            .await?;

            git_ops::create_pull_request(
                &pr.title,
                pr.body.as_deref(),
                pr.draft,
                &pr.branch,
                pr.base.as_deref(),
                &task.id.to_string(),
                &self.engine.workflow_run_config().execution.target_path,
            )
            .await
        }
        .await
        {
            Ok(pr_url) => pr_url,
            Err(error) => {
                let _ = self
                    .engine
                    .append_task_log(
                        task_id,
                        format!("Branch publication and pull request creation failed: {error}"),
                    )
                    .await;
                let _ = self
                    .engine
                    .append_task_log(
                        task_id,
                        "Use create-pr to retry after fixing the remote or permissions",
                    )
                    .await;
                self.engine.emit_error(format!(
                    "Task {} ({}) branch publication/PR creation failed: {}",
                    task.id, node.id, error
                ));
                return Ok(None);
            }
        };

        match &pr_url {
            Some(pr_url) => {
                let _ = self
                    .engine
                    .append_task_log(task_id, format!("Pull request created: {}", pr_url))
                    .await;
            }
            None => {
                let _ = self
                    .engine
                    .append_task_log(task_id, "Pull request created successfully")
                    .await;
            }
        }

        Ok(pr_url)
    }

    pub(crate) async fn prepare_task_worktree(
        engine: &mut Engine,
        task_id: Uuid,
        task: &Task,
        workflow_run: &WorkflowRun,
        node: &Node,
        cleanup_slot: &WorktreeCleanup,
    ) -> Result<()> {
        if !engine.workflow_run_config().managed_git.enable_worktrees
            || !should_manage_git_for_node(
                node,
                engine.workflow_run_config().managed_git.enable_managed_git,
            )
        {
            return Ok(());
        }

        let resolved_params = resolve_workflow_run_params(workflow_run);
        let ctx = git_ops::build_task_expression_context(&task.id.to_string());
        let configured_branch = node.branch_name.as_ref().map(|tmpl| {
            butterflow_models::resolve_string_with_expression(
                tmpl,
                &resolved_params,
                &HashMap::new(),
                task.matrix_values.as_ref(),
                None,
                Some(&ctx),
            )
            .unwrap_or_else(|_| format!("codemod-{}", ctx.signature))
        });
        let branch = git_ops::resolve_branch_name(configured_branch.as_deref(), &ctx.signature);
        let base_target_path = engine.workflow_run_config().execution.target_path.clone();
        let _ = engine
            .append_task_log(
                task_id,
                format!("Resolving git repo root for branch {branch}"),
            )
            .await;

        let repo_root = match tokio::time::timeout(
            tokio::time::Duration::from_secs(15),
            git_ops::repo_root(&base_target_path),
        )
        .await
        {
            Err(_) => {
                return Err(Error::Runtime(format!(
                    "Timed out resolving repo root for git worktree on branch {branch}"
                )))
            }
            Ok(Err(error)) => {
                return Err(Error::Runtime(format!(
                    "Failed to resolve repo root for git worktree: {}",
                    error
                )))
            }
            Ok(Ok(repo_root)) => repo_root,
        };

        let _ = engine
            .append_task_log(
                task_id,
                format!(
                    "Creating git worktree for branch {} in {}",
                    branch,
                    repo_root.display()
                ),
            )
            .await;

        let worktree_path = match tokio::time::timeout(
            tokio::time::Duration::from_secs(120),
            git_ops::create_worktree(&repo_root, &branch, &task.id.to_string()),
        )
        .await
        {
            Err(_) => {
                return Err(Error::Runtime(format!(
                    "Timed out creating git worktree for branch {branch}"
                )))
            }
            Ok(Err(error)) => {
                return Err(Error::Runtime(format!(
                    "Failed to prepare git worktree: {}",
                    error
                )))
            }
            Ok(Ok(worktree_path)) => worktree_path,
        };

        engine.workflow_run_config_mut().execution.target_path = worktree_path.clone();
        engine
            .workflow_run_config_mut()
            .managed_git
            .managed_git_worktree = Some(ManagedGitWorktree {
            branch,
            path: worktree_path.clone(),
        });
        let _ = engine
            .append_task_log(
                task_id,
                format!("Git worktree ready at {}", worktree_path.display()),
            )
            .await;
        if let Ok(mut cleanup) = cleanup_slot.lock() {
            *cleanup = Some((repo_root, worktree_path));
        }

        Ok(())
    }

    pub(crate) async fn cleanup_worktree(
        &self,
        task_id: Uuid,
        cleanup_slot: &WorktreeCleanup,
        panic_context: bool,
    ) {
        let worktree_cleanup = cleanup_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some((repo_root, worktree_path)) = worktree_cleanup {
            if let Err(error) = git_ops::remove_worktree(&repo_root, &worktree_path).await {
                let context = if panic_context {
                    "panicked task"
                } else {
                    "task"
                };
                self.engine.emit_error(format!(
                    "Failed to clean up git worktree for {} {}: {}",
                    context, task_id, error
                ));
            }
        }
    }

    pub(crate) async fn begin_task_branch(
        &self,
        task: &Task,
        node: &Node,
        resolved_params: &HashMap<String, serde_json::Value>,
        task_expr_ctx: &TaskExpressionContext,
    ) -> Result<Option<String>> {
        if !should_manage_git_for_node(
            node,
            self.engine
                .workflow_run_config()
                .managed_git
                .enable_managed_git,
        ) {
            return Ok(None);
        }

        if let Some(worktree) = &self
            .engine
            .workflow_run_config()
            .managed_git
            .managed_git_worktree
        {
            return Ok(Some(worktree.branch.clone()));
        }

        let configured_branch = node.branch_name.as_ref().map(|tmpl| {
            butterflow_models::resolve_string_with_expression(
                tmpl,
                resolved_params,
                &HashMap::new(),
                task.matrix_values.as_ref(),
                None,
                Some(task_expr_ctx),
            )
            .unwrap_or_else(|_| format!("codemod-{}", task_expr_ctx.signature))
        });
        let branch =
            git_ops::resolve_branch_name(configured_branch.as_deref(), &task_expr_ctx.signature);
        git_ops::checkout_branch(
            &branch,
            &self.engine.workflow_run_config().execution.target_path,
        )
        .await?;
        Ok(Some(branch))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn finalize_task(
        &self,
        task_id: Uuid,
        task: &Task,
        node: &Node,
        resolved_params: &HashMap<String, serde_json::Value>,
        managed_branch_name: Option<&String>,
        had_commit_checkpoint: &mut bool,
    ) -> Result<()> {
        let _ = self
            .engine
            .append_task_log(task_id, "Step execution finished; finalizing git state")
            .await;
        let Some(branch) = managed_branch_name else {
            return Ok(());
        };

        let target_path = &self.engine.workflow_run_config().execution.target_path;
        let git_step_logger = self.engine.structured_logger.with_context(StepContext {
            step_name: "Push & create pull request".to_string(),
            step_index: node.steps.len(),
            node_id: node.id.clone(),
            node_name: node.name.clone(),
            task_id: task_id.to_string(),
            step_id: Some("_codemod_auto_push".to_string()),
        });

        git_step_logger.step_start();
        if !git_step_logger.is_jsonl() && !self.engine.workflow_run_config().output.quiet {
            println!("\x1b[1;36m⏺ Push & create pull request\x1b[0m");
        }
        let git_step_start = std::time::Instant::now();

        if !*had_commit_checkpoint {
            let _ = self
                .engine
                .append_task_log(task_id, "Checking worktree for remaining changes")
                .await;
            if let Ok(true) = git_ops::has_changes(target_path).await {
                slog!(
                    git_step_logger,
                    info,
                    "No commit checkpoints in node '{}' but changes detected — creating fallback commit",
                    node.name
                );
                match git_ops::commit(&node.name, &[], true, target_path).await {
                    Ok(true) => *had_commit_checkpoint = true,
                    Ok(false) => {}
                    Err(e) => self
                        .engine
                        .emit_error(format!("Fallback commit failed: {}", e)),
                }
            }
        }

        if !*had_commit_checkpoint {
            let _ = self
                .engine
                .append_task_log(task_id, "No changes detected; no PR created")
                .await;
            slog!(
                git_step_logger,
                info,
                "No changes detected in node '{}' — skipping push and PR creation",
                node.name
            );
            git_step_logger.step_end("success", git_step_start.elapsed().as_millis() as u64);
            return Ok(());
        }

        let _ = self
            .engine
            .append_task_log(task_id, "Publishing branch and creating pull request")
            .await;
        let push_and_pr_result: Result<PullRequestOutcome> = async {
            let pr = self
                .resolve_pull_request_config(task, node, resolved_params)?
                .ok_or_else(|| {
                    Error::Runtime("Task is not eligible for pull request creation".to_string())
                })?;
            let _ = self
                .engine
                .append_task_log(task_id, pull_request_metadata_log_line(&pr))
                .await;

            if let Some(approval_callback) = &self
                .engine
                .workflow_run_config()
                .interaction
                .pull_request_approval_callback
            {
                let approved = approval_callback(&PullRequestCreationRequest {
                    title: pr.title.clone(),
                    body: pr.body.clone(),
                    draft: pr.draft,
                    head: pr.branch.clone(),
                    base: pr.base.clone(),
                    node_id: node.id.clone(),
                    node_name: node.name.clone(),
                    task_id: task.id.to_string(),
                })
                .map_err(|error| Error::Runtime(error.to_string()))?;
                if !approved {
                    let _ = self
                        .engine
                        .append_task_log(
                            task_id,
                            "Branch publication and pull request creation deferred; use create-pr to continue later",
                        )
                        .await;
                    return Ok(PullRequestOutcome::Deferred);
                }
            }

            git_ops::push_branch(branch, target_path).await?;
            git_ops::create_pull_request(
                &pr.title,
                pr.body.as_deref(),
                pr.draft,
                &pr.branch,
                pr.base.as_deref(),
                &task.id.to_string(),
                target_path,
            )
            .await
            .map(PullRequestOutcome::Created)
        }
        .await;

        match &push_and_pr_result {
            Ok(PullRequestOutcome::Created(Some(pr_url))) => {
                slog!(git_step_logger, info, "Pull request created: {}", pr_url);
                let _ = self
                    .engine
                    .append_task_log(task_id, format!("Pull request created: {}", pr_url))
                    .await;
            }
            Ok(PullRequestOutcome::Created(None)) => {
                slog!(git_step_logger, info, "Pull request created successfully");
                let _ = self
                    .engine
                    .append_task_log(task_id, "Pull request created successfully")
                    .await;
            }
            Ok(PullRequestOutcome::Deferred) => {
                slog!(git_step_logger, info, "Pull request creation deferred");
            }
            _ => {}
        }

        if let Err(e) = push_and_pr_result {
            git_step_logger.step_end("failure", git_step_start.elapsed().as_millis() as u64);
            let _ = self
                .engine
                .append_task_log(
                    task_id,
                    format!("Branch publication and pull request creation failed: {e}"),
                )
                .await;
            let _ = self
                .engine
                .append_task_log(
                    task_id,
                    "Use create-pr to retry after fixing the remote or permissions",
                )
                .await;
            self.engine.emit_error(format!(
                "Task {} ({}) branch publication/PR creation failed: {}",
                task_id, node.id, e
            ));
            return Err(e);
        }

        git_step_logger.step_end("success", git_step_start.elapsed().as_millis() as u64);
        Ok(())
    }
}

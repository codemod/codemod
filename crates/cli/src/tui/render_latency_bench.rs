#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use butterflow_models::{
        node::NodeType,
        step::{Step, StepAction, UseJSAstGrep},
        Task, TaskStatus, Workflow, WorkflowRun, WorkflowStatus,
    };
    use chrono::Utc;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use uuid::Uuid;

    use crate::tui::app::{Screen, TaskProgressView, TuiState};
    use crate::tui::screens::render;

    fn benchmark_run_detail_state(task_count: usize) -> TuiState {
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        let mut state = TuiState {
            screen: Screen::RunDetail,
            current_run: Some(WorkflowRun {
                id: run_id,
                workflow: Workflow {
                    version: "1".to_string(),
                    state: None,
                    params: None,
                    templates: vec![],
                    nodes: vec![butterflow_models::Node {
                        id: "apply-transforms".to_string(),
                        name: "Apply AST transformations".to_string(),
                        description: None,
                        r#type: NodeType::Automatic,
                        depends_on: vec![],
                        trigger: None,
                        strategy: None,
                        runtime: None,
                        steps: vec![Step {
                            id: Some("rewrite-imports".to_string()),
                            name: "Rewrite imports".to_string(),
                            action: StepAction::JSAstGrep(UseJSAstGrep {
                                js_file: "transform.js".to_string(),
                                include: None,
                                exclude: None,
                                base_path: None,
                                max_threads: None,
                                dry_run: None,
                                language: None,
                                capabilities: None,
                                semantic_analysis: None,
                            }),
                            env: None,
                            condition: None,
                            commit: None,
                        }],
                        env: Default::default(),
                        branch_name: None,
                        pull_request: None,
                    }],
                },
                status: WorkflowStatus::Running,
                params: Default::default(),
                bundle_path: None,
                tasks: vec![],
                started_at: now - chrono::Duration::minutes(1),
                ended_at: None,
                capabilities: None,
                name: Some("large-shard-workflow.yaml".to_string()),
                target_path: None,
            }),
            ..TuiState::default()
        };

        state.tasks = (0..task_count)
            .map(|index| {
                let task_id = Uuid::new_v4();
                state.task_progress.insert(
                    task_id,
                    TaskProgressView {
                        processed_files: (index % 97) as u64,
                        total_files: Some(100),
                    },
                );
                Task {
                    id: task_id,
                    workflow_run_id: run_id,
                    node_id: "apply-transforms".to_string(),
                    status: if index % 5 == 0 {
                        TaskStatus::Completed
                    } else {
                        TaskStatus::Running
                    },
                    started_at: Some(now - chrono::Duration::seconds((index % 120) as i64)),
                    ended_at: if index % 5 == 0 { Some(now) } else { None },
                    logs: vec![
                        "Starting js-ast-grep file loop (explicit-files, target files: 100)"
                            .to_string(),
                        format!("Processing file: packages/app-{index}/src/index.ts"),
                    ],
                    master_task_id: None,
                    matrix_values: Some(HashMap::from([(
                        "name".to_string(),
                        serde_json::Value::String(format!(
                            "backstage-package-with-long-shard-name-{index:04}"
                        )),
                    )])),
                    is_master: false,
                    error: None,
                }
            })
            .collect();
        state
    }

    #[test]
    #[ignore = "timed benchmark for the TUI perf workflow"]
    fn large_task_list_render_latency_benchmark() {
        let state = benchmark_run_detail_state(1_000);
        let mut samples_micros = Vec::new();

        for _ in 0..31 {
            let backend = TestBackend::new(120, 40);
            let mut terminal = Terminal::new(backend).expect("test backend should initialize");
            let started_at = Instant::now();
            terminal
                .draw(|frame| render(frame, &state))
                .expect("render should succeed");
            samples_micros.push(started_at.elapsed());
        }

        samples_micros.sort_unstable();
        let median = samples_micros[samples_micros.len() / 2].as_micros();
        let min = samples_micros[0].as_micros();
        let max = samples_micros[samples_micros.len() - 1].as_micros();
        let samples = samples_micros
            .iter()
            .map(Duration::as_micros)
            .map(|sample| sample.to_string())
            .collect::<Vec<_>>()
            .join(",");

        println!("TUI_RENDER_LATENCY_MICROS median={median} min={min} max={max} samples={samples}");
    }
}

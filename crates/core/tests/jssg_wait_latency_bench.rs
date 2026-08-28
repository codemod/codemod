use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use butterflow_core::engine::{
    await_js_ast_grep_execution_task, record_unit_progress, StepPhase, StepProgressState,
};
use codemod_sandbox::sandbox::engine::{CodemodOutput, ExecutionResult};
use tokio::sync::Notify;

async fn measure_completion_latency_once() -> Duration {
    let progress_state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
    record_unit_progress(&progress_state, "src/fast.ts", StepPhase::ExecutionStarted);
    let idle_timed_out = Arc::new(AtomicBool::new(false));
    let idle_notify = Arc::new(Notify::new());
    let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));

    let local = tokio::task::LocalSet::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        local.run_until(async move {
            let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

            let execution_task = tokio::task::spawn_local(async move {
                let _ = release_rx.await;
                Ok(CodemodOutput {
                    primary: ExecutionResult::Unmodified,
                    secondary: vec![],
                })
            });

            let wait_task = tokio::task::spawn_local(await_js_ast_grep_execution_task(
                execution_task,
                Arc::clone(&idle_timed_out),
                Arc::clone(&idle_notify),
                Arc::clone(&idle_failure_message),
                Arc::clone(&progress_state),
                Duration::from_secs(1),
                "src/fast.ts",
            ));

            tokio::task::yield_now().await;
            let started_at = Instant::now();
            release_tx
                .send(())
                .expect("completion signal should be sent");
            let output = wait_task
                .await
                .expect("wait task should join")
                .expect("helper should return successfully")
                .expect("execution should complete successfully");
            assert!(matches!(output.primary, ExecutionResult::Unmodified));
            started_at.elapsed()
        }),
    )
    .await
    .expect("benchmark sample exceeded timeout")
}

#[tokio::test]
#[ignore = "timed benchmark for the TUI perf workflow"]
async fn await_js_ast_grep_execution_task_completion_latency_benchmark() {
    let mut samples_micros = Vec::new();
    for _ in 0..21 {
        samples_micros.push(measure_completion_latency_once().await.as_micros());
    }

    samples_micros.sort_unstable();
    let median = samples_micros[samples_micros.len() / 2];
    let min = samples_micros[0];
    let max = samples_micros[samples_micros.len() - 1];
    let samples = samples_micros
        .iter()
        .map(|sample| sample.to_string())
        .collect::<Vec<_>>()
        .join(",");

    println!("JSSG_WAIT_LATENCY_MICROS median={median} min={min} max={max} samples={samples}");
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::tui::app::tests::benchmark_run_detail_state;
    use crate::tui::screens::render;

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

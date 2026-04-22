# TUI Performance Baseline

Use this note for observed TUI performance baselines. Keep machine-specific numbers here instead of in CI assertions.

## Deterministic Baseline

The stable redraw behavior is locked down by tests in [crates/cli/src/tui/mod.rs](/Users/sahilmobaidin/Desktop/myprojects/codemod/crates/cli/src/tui/mod.rs):

- `should_redraw_is_false_for_static_idle_ui_without_changes`
- `should_redraw_is_true_when_deadline_is_due`
- `draw_counter_counts_only_initial_draw_for_static_idle_screen`
- `draw_counter_counts_one_redraw_for_burst_of_drained_workflow_events`
- `draw_counter_counts_deadline_tick_for_running_screen`
- `reduce_workflow_receiver_requests_snapshot_after_receiver_lag`

These tests are the regression baseline for redraw eligibility, redraw counts, and snapshot reconcile behavior.

Current status:

- Established and passing locally on `feature/workflow-tui-rewrite`
- Latest confirmation included successful runs of:
  - `should_redraw_is_false_for_static_idle_ui_without_changes`
  - `draw_counter_counts_only_initial_draw_for_static_idle_screen`
  - `draw_counter_counts_one_redraw_for_burst_of_drained_workflow_events`
  - `draw_counter_counts_deadline_tick_for_running_screen`
  - `reduce_workflow_receiver_requests_snapshot_after_receiver_lag`

## Runtime Counters

Set `CODEMOD_TUI_PERF_METRICS=1` when launching the workflow TUI.

After the TUI exits, it prints a summary like:

```text
TUI perf counters: draws=... workflow_events=... workflow_lag_reconciles=... ui_events=... terminal_events=... terminal_key_events=... terminal_resize_events=... deadline_wakeups=...
```

Counters:

- `draws`: terminal draw calls
- `workflow_events`: workflow events applied to TUI state
- `workflow_lag_reconciles`: full snapshot reloads triggered by broadcast lag
- `ui_events`: internal UI events applied to TUI state
- `terminal_events`: total terminal input events observed
- `terminal_key_events`: terminal key events observed
- `terminal_resize_events`: terminal resize events observed
- `deadline_wakeups`: timer wakeups caused by redraw deadlines

For CI-based perf runs:

- use the manual workflow in [tui-perf.yml](/Users/sahilmobaidin/Desktop/myprojects/codemod/.github/workflows/tui-perf.yml)
- the workflow also runs automatically on pull requests and compares the PR base SHA against the PR head SHA
- target a dedicated self-hosted runner label such as `perf`
- do not share the perf workflow with the normal `[self-hosted, Linux, X64]` CI pool if you want stable numbers
- the preferred workflow input is `pr_number`, which compares that PR's base SHA vs head SHA automatically
- `baseline_ref` and `candidate_ref` remain available as manual overrides for non-PR comparisons
- the workflow generates its own completed, awaiting-trigger, and active sample runs in isolated temporary state, then cancels any non-completed runs during cleanup

## Scenarios To Record

Record baselines for these scenarios over a fixed run:

1. Static completed run attached for 10 seconds
2. Awaiting-trigger run attached for 10 seconds
3. Active run attached for 10 seconds

## Suggested Baseline Table

Fill in this table with real measurements when comparing future changes:

| Date | Commit | Machine | Scenario | Draws | Workflow Events | Snapshot Reconciles | Deadline Wakeups | Notes |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |
|  |  |  | Static completed run (10s) |  |  |  |  |  |
|  |  |  | Awaiting-trigger run (10s) |  |  |  |  |  |
|  |  |  | Active run (10s) |  |  |  |  |  |

## Expectations

- Static and awaiting-trigger screens should draw once initially, then stay quiet unless input arrives.
- `workflow_lag_reconciles` should stay at `0` during healthy runs.
- Active runs should be driven by real workflow events plus deadline ticks for elapsed clocks, not by background polling.

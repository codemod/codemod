# Runtime benchmark environment

Recorded 2026-11-06T02:09:42Z. hyperfine --warmup 1 --runs 5.

- Model: Mac16,7 (MacBook Pro)
- CPU: Apple M4 Pro, 14 (10 Performance and 4 Efficiency)
- RAM: 48 GB
- OS: macOS 26.5.2 (25F84), arm64
- node: v24.18.0; codemod: codemod 1.13.18; hyperfine: hyperfine 1.20.0
- codemod repo: 6d0ad87 + benchmark harness; vite: v8.3.6 (0fd9a29f57d49d496f39c952f44c1b5fc3e90769)

## Arms

- `yaml`: js-ast-grep steps run one after another; each walks and transforms files on min(available_parallelism=14, 12) = 12 threads (no max_threads), one QuickJS runtime per file
  ```sh
  /tmp/claude-501/rt-bench-target/release/codemod workflow run --workflow /tmp/claude-501/rt-bench-stage/benchmarks/runtimes/yaml/workflow.yaml --target /tmp/claude-501/codemod-bench/vite2 --no-interactive --disable-analytics
  ```
- `ts-bridge`: sequence(): one command at a time; each jssg command is one bridge process transforming its batch serially on 1 thread, one QuickJS runtime per file (file selection/reads/writes in Node)
  ```sh
  node /tmp/claude-501/rt-bench-stage/packages/orchestration/bin/codemod-workflow.mjs /tmp/claude-501/rt-bench-stage/benchmarks/runtimes/ts/workflow.ts --target /tmp/claude-501/codemod-bench/vite2 --bridge /tmp/claude-501/rt-bench-target/release/butterflow-execution-bridge
  ```
- `ts-node`: skipped (not available)
- `ts-quickjs`: skipped (not available)

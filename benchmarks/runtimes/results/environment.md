# Runtime benchmark environment

Recorded 2026-11-06T23:17:29Z. hyperfine --warmup 2 --runs 10.

- Model: Mac16,7
- CPU: Apple M4 Pro, 14 cores (10 performance + 4 efficiency), 14 logical
- RAM: 48 GiB
- OS: macOS 26.5.2 (25F84), arm64
- node: v24.18.0; codemod: codemod 1.13.18; hyperfine: hyperfine 1.20.0
- codemod repo: a3aa624; vite: v8.3.6 (0fd9a29f57d49d496f39c952f44c1b5fc3e90769)

## Arms

- `yaml`: js-ast-grep steps run one after another; each walks and transforms files on min(available_parallelism=14, 12) = 12 threads (no max_threads), one QuickJS runtime per file
  ```sh
  perl -e 'alarm shift; exec @ARGV or die' 120 /Users/devagr/codemod/codemod-rt-integration/target/release/codemod workflow run --workflow /Users/devagr/codemod/codemod-rt-integration/benchmarks/runtimes/yaml/workflow.yaml --target /Users/devagr/.cache/codemod-bench/vite-final2 --no-interactive --disable-analytics
  ```
- `ts-bridge`: sequence(): one command at a time; each jssg command is one bridge process transforming its batch serially on 1 thread, one QuickJS runtime per file (file selection/reads/writes in Node)
  ```sh
  perl -e 'alarm shift; exec @ARGV or die' 120 node /Users/devagr/codemod/codemod-rt-integration/packages/orchestration/bin/codemod-workflow.mjs run --runtime=bridge /Users/devagr/codemod/codemod-rt-integration/benchmarks/runtimes/ts/workflow.ts --target /Users/devagr/.cache/codemod-bench/vite-final2 --bridge /Users/devagr/codemod/codemod-rt-integration/target/release/butterflow-execution-bridge
  ```
- `ts-node`: one Node process; workflow and every transform on the single JS thread, serially per file (the napi addon walks/reads the selection natively and parses and selector-matches 64-file chunks on worker threads, one chunk ahead)
  ```sh
  perl -e 'alarm shift; exec @ARGV or die' 120 node /Users/devagr/codemod/codemod-rt-integration/packages/orchestration/bin/codemod-workflow.mjs run --runtime=node /Users/devagr/codemod/codemod-rt-integration/benchmarks/runtimes/ts/workflow.ts --target /Users/devagr/.cache/codemod-bench/vite-final2
  ```
- `ts-quickjs`: one QuickJS context in one Rust process (launched by Node, which reuses the cached bundle); workflow and every transform on the single JS thread, serially per file (native walk/reads on worker threads; Rust parses each file on the JS thread)
  ```sh
  perl -e 'alarm shift; exec @ARGV or die' 120 node /Users/devagr/codemod/codemod-rt-integration/packages/orchestration/bin/codemod-workflow.mjs run --runtime=quickjs /Users/devagr/codemod/codemod-rt-integration/benchmarks/runtimes/ts/workflow.ts --target /Users/devagr/.cache/codemod-bench/vite-final2
  ```

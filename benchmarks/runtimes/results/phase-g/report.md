# Runtime benchmark report

## Provenance

- Recorded: 2026-11-07T11:50:42.004Z
- codemod repo: 7266b91f798763419c8c98206d6f0d698285470b (dirty) on rt/integration
- Input `probe`: probe (make_probe_repo) at 93dd13efc4f57052f9eea253c77f6a63f79bcf3b
- Input `vite`: vite v8.3.6 at 0fd9a29f57d49d496f39c952f44c1b5fc3e90769
- Input `vite-x4`: vite-x4 (vite v8.3.6) at 758b3161a3ee291ba34bf553f45c578ede093a57
- Input `vite-x16`: vite-x16 (vite v8.3.6) at 624b067f8e8e61bc0e9d10e8761ea90fcc3eb92a
- Cache mode: warm
- Versions: node v24.11.1; rustc rustc 1.96.0 (ac68faa20 2026-05-25); hyperfine hyperfine 1.19.0; codemod codemod 1.13.18
- `codemod`: /workspace/codemod/target/release/codemod sha256 505b08d70674199ebfc7cc981cba985e743ab18cafefa81206905ff59a8a28fa
- `butterflow-execution-bridge`: /workspace/codemod/target/release/butterflow-execution-bridge sha256 e16017925248abc88c5536e42e2537a5e7a970b56c68075313b29667ed37b7cd
- `orchestration-quickjs-host`: /workspace/codemod/target/release/orchestration-quickjs-host sha256 2b1e6910001348fb8d3f5a87e11e8488196fb5cb697de9572532e5128836d10a
- `jssg-napi`: /workspace/codemod/packages/jssg-napi/jssg-napi.linux-x64.node sha256 03c9d9d7735643ad33bc7d24da8429fa3449d888f09c5f0493a9cea680c14fce
- `bench-exec`: /workspace/codemod/benchmarks/runtimes/bin/bench-exec sha256 02fc16aab96ae1f43aa5cb9c109e3e9b7e6fbbdc4b54df333bbdea36b329a700
- arm `yaml`: `/workspace/codemod/benchmarks/runtimes/bin/bench-exec -t 600 /workspace/codemod/target/release/codemod workflow run --workflow /workspace/codemod/benchmarks/runtimes/yaml/workflow.yaml --target $REPO --no-interactive --disable-analytics`
- arm `ts-bridge`: `/workspace/codemod/benchmarks/runtimes/bin/bench-exec -t 600 node /workspace/codemod/packages/orchestration/bin/codemod-workflow.mjs run --runtime=bridge /workspace/codemod/benchmarks/runtimes/ts/workflow.ts --target $REPO --bridge /workspace/codemod/target/release/butterflow-execution-bridge`
- arm `ts-node`: `/workspace/codemod/benchmarks/runtimes/bin/bench-exec -t 600 node /workspace/codemod/packages/orchestration/bin/codemod-workflow.mjs run --runtime=node /workspace/codemod/benchmarks/runtimes/ts/workflow.ts --target $REPO`
- arm `ts-quickjs`: `/workspace/codemod/benchmarks/runtimes/bin/bench-exec -t 600 node /workspace/codemod/packages/orchestration/bin/codemod-workflow.mjs run --runtime=quickjs /workspace/codemod/benchmarks/runtimes/ts/workflow.ts --target $REPO`
- Analysis: 10000 bootstrap resamples, seed 1; noise threshold IQR/median <= 5.0 %

## Wall time: probe (2 files, 10 rounds, warm cache; gate 1 files, 706954532705)

| Arm | n | Median [s] | IQR [s] | IQR/median | Min [s] | Max [s] | User [s] | System [s] | vs `yaml` (95 % CI) |
|:---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `yaml` | 10 | 0.099 | 0.028 | 28.0 % | 0.086 | 0.128 | 0.050 | 0.049 | 1.00 [1.00, 1.00] |
| `ts-node` | 10 | 0.121 | 0.011 | 9.3 % | 0.099 | 0.125 | 0.104 | 0.058 | 1.22 [0.99, 1.38] |
| `ts-quickjs` | 10 | 0.130 | 0.014 | 10.9 % | 0.108 | 0.142 | 0.096 | 0.055 | 1.32 [1.07, 1.52] |
| `ts-bridge` | 10 | 0.160 | 0.025 | 15.4 % | 0.141 | 0.177 | 0.137 | 0.072 | 1.62 [1.29, 1.90] |

## Wall time: vite (1615 files, 10 rounds, warm cache; gate 331 files, 8f7ff40f81b3)

| Arm | n | Median [s] | IQR [s] | IQR/median | Min [s] | Max [s] | User [s] | System [s] | vs `ts-node` (95 % CI) |
|:---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `ts-node` | 10 | 1.002 | 0.043 | 4.3 % | 0.921 | 1.050 | 1.580 | 0.427 | 1.00 [1.00, 1.00] |
| `ts-quickjs` | 10 | 1.392 | 0.029 | 2.1 % | 1.354 | 1.446 | 1.260 | 0.268 | 1.39 [1.36, 1.46] |
| `yaml` | 10 | 1.463 | 0.030 | 2.0 % | 1.437 | 1.548 | 4.105 | 0.598 | 1.46 [1.44, 1.54] |
| `ts-bridge` | 10 | 4.161 | 0.058 | 1.4 % | 4.056 | 4.218 | 3.912 | 0.434 | 4.15 [4.08, 4.35] |

## Wall time: vite-x4 (6460 files, 10 rounds, warm cache; gate 1324 files, 90fa72712243)

| Arm | n | Median [s] | IQR [s] | IQR/median | Min [s] | Max [s] | User [s] | System [s] | vs `ts-node` (95 % CI) |
|:---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `ts-node` | 10 | 3.378 | 0.147 | 4.4 % | 3.239 | 3.616 | 5.678 | 1.157 | 1.00 [1.00, 1.00] |
| `ts-quickjs` | 10 | 4.926 | 0.048 | 1.0 % | 4.841 | 5.151 | 4.605 | 0.647 | 1.46 [1.43, 1.51] |
| `yaml` | 10 | 5.794 | 0.110 | 1.9 % | 5.596 | 6.003 | 16.519 | 2.323 | 1.72 [1.67, 1.77] |
| `ts-bridge` | 10 | 16.043 | 0.067 | 0.4 % | 15.896 | 16.147 | 15.198 | 1.365 | 4.75 [4.65, 4.91] |

## Wall time: vite-x16 (25840 files, 10 rounds, warm cache; gate 5296 files, 3c1e4dd6caa9)

| Arm | n | Median [s] | IQR [s] | IQR/median | Min [s] | Max [s] | User [s] | System [s] | vs `ts-node` (95 % CI) |
|:---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `ts-node` | 10 | 13.359 | 0.322 | 2.4 % | 13.095 | 14.368 | 21.585 | 4.848 | 1.00 [1.00, 1.00] |
| `ts-quickjs` | 10 | 20.102 | 1.486 | 7.4 % | 19.675 | 25.848 | 18.801 | 2.498 | 1.50 [1.48, 1.64] |
| `yaml` | 10 | 25.794 | 0.494 | 1.9 % | 25.541 | 32.551 | 68.042 | 10.592 | 1.93 [1.91, 2.00] |
| `ts-bridge` | 10 | 65.603 | 1.079 | 1.6 % | 63.981 | 70.643 | 60.943 | 5.714 | 4.91 [4.83, 5.02] |

## Scaling: wall = fixed + per_file x N

| Arm | Fixed [s] (95 % CI) | Per file [ms] (95 % CI) | R^2 | Points (files: median s) |
|:---|---:|---:|---:|:---|
| `yaml` | -0.207 [-0.273, -0.186] | 1.0018 [0.9965, 1.0304] | 0.9992 | 2: 0.099, 1615: 1.463, 6460: 5.794, 25840: 25.794 |
| `ts-bridge` | -0.015 [-0.071, 0.072] | 2.5364 [2.5002, 2.5539] | 0.9999 | 2: 0.160, 1615: 4.161, 6460: 16.043, 25840: 65.603 |
| `ts-node` | 0.126 [0.088, 0.161] | 0.5117 [0.5013, 0.5184] | 1.0000 | 2: 0.121, 1615: 1.002, 6460: 3.378, 25840: 13.359 |
| `ts-quickjs` | 0.078 [-0.088, 0.113] | 0.7736 [0.7603, 0.8440] | 0.9999 | 2: 0.130, 1615: 1.392, 6460: 4.926, 25840: 20.102 |

```text
    65.6 s |                                                           B
           |                                                            
           |                                                           Y
           |                                                  B        Q
           |                                                           N
           |                                                            
           |                                          B       *         
           |                                                  N         
    2.05 s |                                                            
           |                                          *                 
           |                                          N                 
           |                                                            
           |                                                            
           |                                                            
           |*                                                           
  0.0988 s |*                                                           
           +------------------------------------------------------------
            2 files                                          25840 files
            (log-log; Y = yaml, B = ts-bridge, N = ts-node, Q = ts-quickjs; * = overlap)
```

## Phases: probe (median ms per run, all processes merged)

| Phase | `yaml` | `ts-bridge` | `ts-node` | `ts-quickjs` |
|:---|---:|---:|---:|---:|
| boot | 11.7 | 60.1 | 57.9 | 59.3 |
| plan | 39.7 | 21.8 | 20.6 | 16.6 |
| select | 12.1 | 9.0 | 7.8 | 11.9 |
| read | 0.0 | 0.6 | 0.4 | 0.5 |
| setup | 20.5 | 29.3 | 0.7 | 12.4 |
| parse | 0.1 | 0.6 | 1.7 | 0.7 |
| transform | 0.3 | 0.8 | 0.6 | 0.6 |
| convert | 0.0 | 3.2 | 0.1 | 0.0 |
| commit | 0.3 | 0.6 | 0.4 | 0.5 |
| idle_wait | 0.1 | n/a | n/a | n/a |
| conflict | n/a | 1.2 | 0.5 | 0.4 |
| **coverage** (phases / in-process wall) | 96.1 % | 94.0 % | 94.7 % | 96.1 % |
| exec_to_main | 7.9 | 7.8 | 9.2 | 7.4 |
| in-process wall | 85.1 | 136.3 | 94.9 | 106.5 |
| after exit hook (hyperfine − exec_to_main − in-process) | 6.1 | 16.3 | 14.2 | 15.7 |
| hyperfine wall | 98.8 | 160.4 | 120.6 | 130.0 |
| poll lag (poll_saw − engine_done) | 0.1 | n/a | n/a | n/a |
| processes | 1 | 3 | 1 | 2 |
| runs with timings | 10/10 | 10/10 | 10/10 | 10/10 |

## Phases: vite (median ms per run, all processes merged)

| Phase | `yaml` | `ts-bridge` | `ts-node` | `ts-quickjs` |
|:---|---:|---:|---:|---:|
| boot | 13.3 | 60.5 | 59.5 | 62.6 |
| plan | 162.9 | 84.1 | 20.7 | 17.2 |
| select | 124.4 | 226.2 | 99.1 | 124.9 |
| read | 17.1 | 87.3 | 28.7 | 29.3 |
| setup | 823.7 | 2441.8 | 15.3 | 32.7 |
| parse | 177.7 | 623.5 | 289.3 | 698.5 |
| transform | 103.6 | 376.2 | 340.7 | 301.1 |
| convert | 0.5 | 14.2 | 8.0 | 13.3 |
| commit | 32.1 | 40.4 | 23.0 | 27.1 |
| idle_wait | 0.1 | n/a | n/a | n/a |
| conflict | n/a | 26.6 | 15.7 | 53.2 |
| **coverage** (phases / in-process wall) | 99.8 % | 96.5 % | 99.2 % | 99.5 % |
| exec_to_main | 8.3 | 6.7 | 6.3 | 7.9 |
| in-process wall | 1448.8 | 4118.1 | 928.2 | 1367.5 |
| after exit hook (hyperfine − exec_to_main − in-process) | 6.4 | 28.3 | 72.9 | 15.5 |
| hyperfine wall | 1463.0 | 4160.8 | 1001.7 | 1392.2 |
| poll lag (poll_saw − engine_done) | 0.1 | n/a | n/a | n/a |
| processes | 1 | 3 | 1 | 2 |
| runs with timings | 10/10 | 10/10 | 10/10 | 10/10 |

## Phases: vite-x4 (median ms per run, all processes merged)

| Phase | `yaml` | `ts-bridge` | `ts-node` | `ts-quickjs` |
|:---|---:|---:|---:|---:|
| boot | 12.8 | 62.3 | 63.3 | 67.7 |
| plan | 746.6 | 271.3 | 21.0 | 16.2 |
| select | 413.4 | 803.5 | 150.5 | 165.4 |
| read | 74.0 | 297.7 | 101.9 | 73.3 |
| setup | 3261.2 | 9838.6 | 59.2 | 90.1 |
| parse | 700.1 | 2487.4 | 1379.6 | 2829.0 |
| transform | 415.3 | 1514.5 | 1335.9 | 1219.8 |
| convert | 2.0 | 34.0 | 32.8 | 57.5 |
| commit | 140.9 | 79.9 | 84.1 | 112.6 |
| idle_wait | 0.2 | n/a | n/a | n/a |
| conflict | n/a | 53.4 | 55.9 | 252.7 |
| **coverage** (phases / in-process wall) | 99.9 % | 96.7 % | 99.7 % | 99.7 % |
| exec_to_main | 7.6 | 8.1 | 6.5 | 7.9 |
| in-process wall | 5775.8 | 16003.7 | 3297.5 | 4902.1 |
| after exit hook (hyperfine − exec_to_main − in-process) | 7.9 | 34.4 | 67.0 | 14.8 |
| hyperfine wall | 5793.8 | 16042.6 | 3377.7 | 4926.5 |
| poll lag (poll_saw − engine_done) | 0.2 | n/a | n/a | n/a |
| processes | 1 | 3 | 1 | 2 |
| runs with timings | 10/10 | 10/10 | 10/10 | 10/10 |

## Phases: vite-x16 (median ms per run, all processes merged)

| Phase | `yaml` | `ts-bridge` | `ts-node` | `ts-quickjs` |
|:---|---:|---:|---:|---:|
| boot | 18.6 | 61.7 | 68.9 | 75.2 |
| plan | 5916.9 | 1041.1 | 23.0 | 17.2 |
| select | 1558.7 | 3106.4 | 679.4 | 711.3 |
| read | 323.5 | 1270.9 | 420.7 | 307.7 |
| setup | 13014.1 | 40672.0 | 237.7 | 335.2 |
| parse | 2747.1 | 10163.7 | 5449.2 | 11775.0 |
| transform | 1658.4 | 6196.8 | 5374.9 | 5009.8 |
| convert | 7.7 | 132.2 | 126.2 | 245.7 |
| commit | 577.8 | 384.3 | 376.1 | 473.4 |
| idle_wait | 0.1 | n/a | n/a | n/a |
| conflict | n/a | 184.6 | 233.1 | 1064.8 |
| **coverage** (phases / in-process wall) | 100.0 % | 96.4 % | 98.9 % | 99.8 % |
| exec_to_main | 19.9 | 7.3 | 9.7 | 8.0 |
| in-process wall | 25761.7 | 65526.1 | 13204.3 | 20073.8 |
| after exit hook (hyperfine − exec_to_main − in-process) | 12.5 | 68.1 | 152.2 | 16.0 |
| hyperfine wall | 25793.9 | 65602.5 | 13359.1 | 20101.8 |
| poll lag (poll_saw − engine_done) | 0.1 | n/a | n/a | n/a |
| processes | 1 | 3 | 1 | 2 |
| runs with timings | 10/10 | 10/10 | 10/10 | 10/10 |

## Noise check

FAILED: IQR/median above 5.0 %:

- probe `yaml`: 28.0 %
- probe `ts-bridge`: 15.4 %
- probe `ts-node`: 9.3 %
- probe `ts-quickjs`: 10.9 %
- vite-x16 `ts-quickjs`: 7.4 %

## Preflight

```text
## Machine

- CPU: AMD EPYC 9334 32-Core Processor, 4 logical
- RAM: 7.8Gi
- OS: Linux 6.12.61-hardened1 x86_64
- node: v24.11.1; codemod: codemod 1.13.18; hyperfine: hyperfine 1.19.0
- codemod repo: 7266b91f; vite: v8.3.6 (0fd9a29f57d49d496f39c952f44c1b5fc3e90769)
- rustc: rustc 1.96.0 (ac68faa20 2026-05-25)
- codemod worktree: 7266b91f798763419c8c98206d6f0d698285470b (rt/integration), dirty=true
- bench repo: /workspace/.cache/codemod-bench/vite at 0fd9a29f57d49d496f39c952f44c1b5fc3e90769

## Checks (mode: warn)

WARN  power/thermal checks are macOS-only
PASS  load: 1-min 0.52 (< 2)
PASS  no other process over 20% CPU
WARN  codemod worktree has uncommitted changes (results would not match a commit) (BENCH_PREFLIGHT=warn)
PASS  yaml: /workspace/codemod/target/release/codemod
PASS  ts-bridge: /workspace/codemod/target/release/butterflow-execution-bridge
PASS  ts-node: /workspace/codemod/packages/jssg-napi/jssg-napi.linux-x64.node
PASS  ts-quickjs: /workspace/codemod/target/release/orchestration-quickjs-host
PASS  tool: node
PASS  tool: hyperfine
PASS  tool: cc
PASS  tool: git
PASS  tool: perl

preflight passed
```

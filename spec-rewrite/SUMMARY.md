# Runtime tests as one-rule specs (commit on local/ts-orch-quickjs-only)

| File | Lines | Cases | Test time (median of 3) |
| --- | --- | --- | --- |
| jssg.test.ts | 345 -> 334 | 15 -> 21 (1 skipped as root, as before) | 250 ms -> 50 ms |
| protocol.test.ts | 240 -> 207 | 36 -> 74 | 9 ms -> 12 ms |
| agent-runner.test.ts | 288 -> 287 | 11 -> 35 | 9.1 s -> 4.8 s |
| scheduler.test.ts | 577 -> 556 | 33 -> 54 | 43 ms -> 49 ms |
| agent.test.ts | 615 -> 592 | 17 -> 97 | 81 ms -> 64 ms |
| target.test.ts | 448 -> 423 | 32 -> 73 | 34 ms -> 35 ms |
| **Total** | **2,513 -> 2,399** | **144 -> 354** | **about 9.5 s -> 5.0 s** |

Verbose before/after output per file: `<file>-before.txt`, `<file>-after.txt`.
Whole package suite: 660 passed, 8 skipped. tsc, oxfmt and oxlint clean.
Every error-scenario test kept. Dropped only constant copies, an environment
check (`getHost().name`), and checks another test now covers.

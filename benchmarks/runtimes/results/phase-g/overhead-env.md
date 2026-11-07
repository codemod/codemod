Mode: env (one build; CODEMOD_TIMINGS_FILE unset vs set)

| Arm | n | Off [s] | On [s] | median(on) / median(off) (95 % CI) | Paired on / off (95 % CI) | Budget 1 % | Timings files |
|:---|---:|---:|---:|---:|---:|:---|---:|
| `ts-bridge` | 20 | 4.138 | 4.146 | 1.0020 [0.9934, 1.0073] | 1.0020 [0.9956, 1.0067] | within | 20/20 |
| `ts-node` | 20 | 0.980 | 0.952 | 0.9713 [0.9401, 1.0150] | 0.9886 [0.9489, 1.0157] | inconclusive | 20/20 |
| `ts-quickjs` | 20 | 1.376 | 1.375 | 0.9996 [0.9873, 1.0245] | 1.0054 [0.9929, 1.0263] | inconclusive | 20/20 |
| `yaml` | 20 | 1.442 | 1.467 | 1.0172 [0.9949, 1.0292] | 0.9968 [0.9902, 1.0233] | inconclusive | 20/20 |

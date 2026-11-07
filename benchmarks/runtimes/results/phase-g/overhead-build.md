Mode: build (off = baseline build without the `timings` feature, on = default build; CODEMOD_TIMINGS_FILE unset in both)

| Arm | n | Off [s] | On [s] | median(on) / median(off) (95 % CI) | Paired on / off (95 % CI) | Budget 1 % | Timings files |
|:---|---:|---:|---:|---:|---:|:---|---:|
| `ts-bridge` | 20 | 4.095 | 4.126 | 1.0075 [0.9955, 1.0153] | 1.0081 [0.9981, 1.0137] | inconclusive | 0/20 |
| `ts-node` | 20 | 0.932 | 0.916 | 0.9832 [0.9562, 1.0376] | 1.0013 [0.9722, 1.0337] | inconclusive | 0/20 |
| `ts-quickjs` | 20 | 1.342 | 1.357 | 1.0112 [0.9763, 1.0215] | 0.9973 [0.9774, 1.0115] | inconclusive | 0/20 |
| `yaml` | 20 | 1.461 | 1.453 | 0.9945 [0.9852, 1.0131] | 0.9960 [0.9869, 1.0117] | inconclusive | 0/20 |

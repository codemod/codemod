
### probe

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| shell-chain | yaml-main | 2.01 s (1.01–2.02) | 0.04 s | 28 MB | 0.02x | 1.0x | yes |
| shell-chain | yaml-signal | 0.04 s (0.04–0.26) | 0.03 s | 28 MB | 1.0x | 54.43x | yes |
| shell-chain | ts-node | 0.12 s (0.11–0.14) | 0.14 s | 70 MB | 0.31x | 16.78x | yes |
| shell-chain | ts-quickjs | 0.12 s (0.10–0.24) | 0.11 s | 56 MB | 0.32x | 17.51x | yes |
| util-is | yaml-main | 2.01 s (1.01–2.01) | 0.04 s | 41 MB | 0.03x | 1.0x | yes |
| util-is | yaml-signal | 0.05 s (0.05–0.20) | 0.04 s | 41 MB | 1.0x | 38.67x | yes |
| util-is | ts-node | 0.09 s (0.08–0.12) | 0.12 s | 75 MB | 0.55x | 21.39x | yes |
| util-is | ts-quickjs | 0.09 s (0.09–0.09) | 0.09 s | 56 MB | 0.58x | 22.6x | yes |
| react-19 | yaml-main | 2.01 s (1.01–2.02) | 0.14 s | 47 MB | 0.07x | 1.0x | yes |
| react-19 | yaml-signal | 0.14 s (0.12–0.36) | 0.15 s | 47 MB | 1.0x | 14.49x | yes |
| react-19 | ts-node | 0.14 s (0.13–0.23) | 0.16 s | 76 MB | 1.03x | 14.92x | yes |
| react-19 | ts-quickjs | 0.12 s (0.12–0.17) | 0.13 s | 56 MB | 1.16x | 16.78x | yes |

### bluebird

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 2.01 s (1.01–2.02) | 1.65 s | 54 MB | 0.3x | 1.0x | yes |
| util-is | yaml-signal | 0.61 s (0.50–0.64) | 1.65 s | 52 MB | 1.0x | 3.32x | yes |
| util-is | ts-node | 0.35 s (0.34–0.50) | 0.93 s | 171 MB | 1.72x | 5.72x | yes |
| util-is | ts-quickjs | 0.23 s (0.22–0.26) | 0.60 s | 56 MB | 2.65x | 8.79x | yes |
| react-19 | yaml-main | 4.30 s (4.26–5.26) | 11.46 s | 70 MB | 0.77x | 1.0x | yes |
| react-19 | yaml-signal | 3.31 s (3.21–3.54) | 11.42 s | 68 MB | 1.0x | 1.3x | yes |
| react-19 | ts-node | 2.07 s (2.03–2.12) | 6.49 s | 288 MB | 1.6x | 2.08x | yes |
| react-19 | ts-quickjs | 1.78 s (1.55–1.83) | 5.42 s | 56 MB | 1.86x | 2.42x | yes |

### react-bootstrap-v1

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| react-19 | yaml-main | 6.28 s (6.25–7.25) | 19.31 s | 51 MB | 0.94x | 1.0x | yes |
| react-19 | yaml-signal | 5.93 s (5.77–6.03) | 19.53 s | 51 MB | 1.0x | 1.06x | yes |
| react-19 | ts-node | 2.45 s (2.08–2.53) | 7.09 s | 224 MB | 2.42x | 2.56x | yes |
| react-19 | ts-quickjs | 1.93 s (1.88–2.02) | 6.26 s | 56 MB | 3.07x | 3.26x | yes |
| debarrel | yaml-main | 2.14 s (2.02–3.02) | 5.93 s | 47 MB | 0.88x | 1.0x | yes |
| debarrel | yaml-signal | 1.89 s (1.87–2.06) | 5.71 s | 47 MB | 1.0x | 1.13x | yes |
| debarrel | ts-node | 0.95 s (0.88–1.18) | 1.91 s | 265 MB | 1.99x | 2.25x | yes |
| debarrel | ts-quickjs | 0.56 s (0.47–0.82) | 1.02 s | 57 MB | 3.37x | 3.82x | yes |

### taxonomy

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 2.01 s (1.01–2.02) | 0.93 s | 44 MB | 0.16x | 1.0x | yes |
| util-is | yaml-signal | 0.33 s (0.31–0.33) | 0.98 s | 44 MB | 1.0x | 6.13x | yes |
| util-is | ts-node | 0.24 s (0.23–0.27) | 0.56 s | 134 MB | 1.38x | 8.45x | yes |
| util-is | ts-quickjs | 0.15 s (0.15–0.27) | 0.34 s | 57 MB | 2.12x | 12.98x | yes |
| react-19 | yaml-main | 2.02 s (2.02–2.22) | 4.91 s | 48 MB | 0.78x | 1.0x | yes |
| react-19 | yaml-signal | 1.57 s (1.46–1.72) | 5.07 s | 49 MB | 1.0x | 1.28x | yes |
| react-19 | ts-node | 0.71 s (0.69–0.71) | 2.10 s | 150 MB | 2.22x | 2.85x | yes |
| react-19 | ts-quickjs | 0.55 s (0.49–0.65) | 1.69 s | 56 MB | 2.84x | 3.65x | yes |
| debarrel | yaml-main | 2.02 s (1.01–2.14) | 1.81 s | 51 MB | 0.29x | 1.0x | yes |
| debarrel | yaml-signal | 0.58 s (0.56–0.80) | 1.85 s | 51 MB | 1.0x | 3.45x | yes |
| debarrel | ts-node | 0.28 s (0.26–0.29) | 0.67 s | 149 MB | 2.12x | 7.33x | yes |
| debarrel | ts-quickjs | 0.20 s (0.20–0.21) | 0.48 s | 56 MB | 2.86x | 9.88x | yes |
| audit | yaml-main | 2.02 s (2.02–4.02) | 4.42 s | 44 MB | 1.03x | 1.0x | yes |
| audit | yaml-signal | 2.08 s (1.85–2.24) | 4.28 s | 44 MB | 1.0x | 0.97x | yes |
| audit | ts-node | 0.58 s (0.55–0.67) | 1.54 s | 167 MB | 3.55x | 3.45x | yes |
| audit | ts-quickjs | 0.42 s (0.38–0.48) | 1.06 s | 56 MB | 4.93x | 4.79x | yes |

### excalidraw

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 2.12 s (2.08–3.02) | 6.50 s | 58 MB | 0.94x | 1.0x | yes |
| util-is | yaml-signal | 1.99 s (1.91–2.09) | 6.57 s | 59 MB | 1.0x | 1.07x | yes |
| util-is | ts-node | 1.23 s (1.14–1.27) | 3.54 s | 323 MB | 1.62x | 1.73x | yes |
| util-is | ts-quickjs | 0.98 s (0.93–1.05) | 2.97 s | 63 MB | 2.02x | 2.15x | yes |
| react-19 | yaml-main | 14.25 s (13.25–14.33) | 45.37 s | 84 MB | 0.89x | 1.0x | yes |
| react-19 | yaml-signal | 12.66 s (11.93–12.81) | 45.12 s | 90 MB | 1.0x | 1.13x | yes |
| react-19 | ts-node | 8.38 s (8.17–8.45) | 27.20 s | 772 MB | 1.51x | 1.7x | yes |
| react-19 | ts-quickjs | 7.11 s (7.01–7.48) | 23.58 s | 92 MB | 1.78x | 2.0x | yes |
| debarrel | yaml-main | 10.25 s (10.23–10.30) | 34.95 s | 113 MB | 0.92x | 1.0x | yes |
| debarrel | yaml-signal | 9.42 s (9.15–9.84) | 33.34 s | 113 MB | 1.0x | 1.09x | yes |
| debarrel | ts-node | 9.66 s (9.38–9.93) | 33.95 s | 1248 MB | 0.97x | 1.06x | yes |
| debarrel | ts-quickjs | 7.64 s (7.40–8.05) | 26.55 s | 122 MB | 1.23x | 1.34x | yes |
| audit | yaml-main | 15.34 s (14.38–16.45) | 38.44 s | 81 MB | 0.96x | 1.0x | yes |
| audit | yaml-signal | 14.66 s (13.84–15.14) | 39.00 s | 82 MB | 1.0x | 1.05x | yes |
| audit | ts-node | 6.40 s (6.25–6.54) | 22.38 s | 818 MB | 2.29x | 2.4x | yes |
| audit | ts-quickjs | 4.82 s (4.64–4.83) | 17.38 s | 115 MB | 3.04x | 3.18x | yes |

### calcom

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 12.24 s (12.22–12.30) | 37.20 s | 219 MB | 0.93x | 1.0x | yes |
| util-is | yaml-signal | 11.39 s (11.24–11.50) | 37.93 s | 219 MB | 1.0x | 1.07x | yes |
| util-is | ts-node | 3.81 s (3.79–3.85) | 12.99 s | 509 MB | 2.99x | 3.21x | yes |
| util-is | ts-quickjs | 3.22 s (3.20–3.59) | 11.88 s | 257 MB | 3.54x | 3.8x | yes |
| debarrel | yaml-main | 33.31 s (33.29–35.31) | 123.14 s | 291 MB | 1.01x | 1.0x | yes |
| debarrel | yaml-signal | 33.54 s (32.81–34.76) | 123.21 s | 290 MB | 1.0x | 0.99x | yes |
| debarrel | ts-node | 21.42 s (21.29–21.71) | 75.72 s | 1394 MB | 1.57x | 1.56x | yes |
| debarrel | ts-quickjs | 16.98 s (16.64–16.98) | 60.64 s | 318 MB | 1.98x | 1.96x | yes |
| audit | yaml-main | 80.38 s (79.37–84.34) | 188.28 s | 64 MB | 0.98x | 1.0x | yes |
| audit | yaml-signal | 78.76 s (78.60–79.32) | 187.76 s | 64 MB | 1.0x | 1.02x | yes |
| audit | ts-node | 17.32 s (17.11–17.44) | 59.70 s | 843 MB | 4.55x | 4.64x | yes |
| audit | ts-quickjs | 13.15 s (12.93–13.56) | 49.88 s | 154 MB | 5.99x | 6.11x | yes |

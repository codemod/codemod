
### probe

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| shell-chain | yaml-main | 2.01 s (1.01–2.02) | 0.04 s | 28 MB | 0.02x | 1.0x | yes |
| shell-chain | yaml-signal | 0.04 s (0.04–0.26) | 0.03 s | 28 MB | 1.0x | 54.43x | yes |
| shell-chain | ts-node | 0.11 s (0.11–0.12) | 0.14 s | 70 MB | 0.32x | 17.67x | yes |
| shell-chain | ts-quickjs | 0.10 s (0.10–0.11) | 0.11 s | 57 MB | 0.36x | 19.37x | yes |
| util-is | yaml-main | 2.01 s (1.01–2.01) | 0.04 s | 41 MB | 0.03x | 1.0x | yes |
| util-is | yaml-signal | 0.05 s (0.05–0.20) | 0.04 s | 41 MB | 1.0x | 38.67x | yes |
| util-is | ts-node | 0.09 s (0.08–0.09) | 0.11 s | 74 MB | 0.59x | 22.85x | yes |
| util-is | ts-quickjs | 0.09 s (0.09–0.11) | 0.09 s | 57 MB | 0.59x | 22.85x | yes |
| react-19 | yaml-main | 2.01 s (1.01–2.02) | 0.14 s | 47 MB | 0.07x | 1.0x | yes |
| react-19 | yaml-signal | 0.14 s (0.12–0.36) | 0.15 s | 47 MB | 1.0x | 14.49x | yes |
| react-19 | ts-node | 0.13 s (0.12–0.14) | 0.15 s | 76 MB | 1.08x | 15.61x | yes |
| react-19 | ts-quickjs | 0.14 s (0.12–0.17) | 0.14 s | 57 MB | 1.03x | 14.92x | yes |

### bluebird

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 2.01 s (1.01–2.02) | 1.65 s | 54 MB | 0.3x | 1.0x | yes |
| util-is | yaml-signal | 0.61 s (0.50–0.64) | 1.65 s | 52 MB | 1.0x | 3.32x | yes |
| util-is | ts-node | 0.58 s (0.51–0.64) | 0.73 s | 124 MB | 1.04x | 3.46x | yes |
| util-is | ts-quickjs | 0.75 s (0.67–0.81) | 0.68 s | 57 MB | 0.81x | 2.67x | yes |
| react-19 | yaml-main | 4.30 s (4.26–5.26) | 11.46 s | 70 MB | 0.77x | 1.0x | yes |
| react-19 | yaml-signal | 3.31 s (3.21–3.54) | 11.42 s | 68 MB | 1.0x | 1.3x | yes |
| react-19 | ts-node | 5.81 s (5.49–6.05) | 6.41 s | 284 MB | 0.57x | 0.74x | yes |
| react-19 | ts-quickjs | 5.89 s (5.81–6.08) | 5.63 s | 57 MB | 0.56x | 0.73x | yes |

### react-bootstrap-v1

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| react-19 | yaml-main | 6.28 s (6.25–7.25) | 19.31 s | 51 MB | 0.94x | 1.0x | yes |
| react-19 | yaml-signal | 5.93 s (5.77–6.03) | 19.53 s | 51 MB | 1.0x | 1.06x | yes |
| react-19 | ts-node | 6.80 s (6.70–6.83) | 7.00 s | 150 MB | 0.87x | 0.92x | yes |
| react-19 | ts-quickjs | 6.77 s (6.32–6.92) | 6.48 s | 57 MB | 0.88x | 0.93x | yes |

### taxonomy

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 2.01 s (1.01–2.02) | 0.93 s | 44 MB | 0.16x | 1.0x | yes |
| util-is | yaml-signal | 0.33 s (0.31–0.33) | 0.98 s | 44 MB | 1.0x | 6.13x | yes |
| util-is | ts-node | 0.29 s (0.24–0.37) | 0.36 s | 82 MB | 1.13x | 6.91x | yes |
| util-is | ts-quickjs | 0.32 s (0.31–0.39) | 0.32 s | 57 MB | 1.03x | 6.33x | yes |
| react-19 | yaml-main | 2.02 s (2.02–2.22) | 4.91 s | 48 MB | 0.78x | 1.0x | yes |
| react-19 | yaml-signal | 1.57 s (1.46–1.72) | 5.07 s | 49 MB | 1.0x | 1.28x | yes |
| react-19 | ts-node | 1.84 s (1.68–1.95) | 1.88 s | 109 MB | 0.85x | 1.1x | yes |
| react-19 | ts-quickjs | 1.69 s (1.62–1.80) | 1.70 s | 57 MB | 0.93x | 1.2x | yes |
| debarrel | yaml-main | 2.02 s (1.01–2.14) | 1.81 s | 51 MB | 0.29x | 1.0x | yes |
| debarrel | yaml-signal | 0.58 s (0.56–0.80) | 1.85 s | 51 MB | 1.0x | 3.45x | yes |
| debarrel | ts-node | 0.35 s (0.34–0.37) | 0.40 s | 101 MB | 1.65x | 5.69x | yes |
| debarrel | ts-quickjs | 0.49 s (0.47–0.55) | 0.49 s | 57 MB | 1.2x | 4.15x | yes |
| audit | yaml-main | 2.02 s (2.02–4.02) | 4.42 s | 44 MB | 1.03x | 1.0x | yes |
| audit | yaml-signal | 2.08 s (1.85–2.24) | 4.28 s | 44 MB | 1.0x | 0.97x | yes |
| audit | ts-node | 0.61 s (0.57–0.62) | 1.08 s | 115 MB | 3.41x | 3.32x | yes |
| audit | ts-quickjs | 1.01 s (0.95–1.30) | 1.01 s | 57 MB | 2.06x | 2.0x | yes |

### excalidraw

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 2.12 s (2.08–3.02) | 6.50 s | 58 MB | 0.94x | 1.0x | yes |
| util-is | yaml-signal | 1.99 s (1.91–2.09) | 6.57 s | 59 MB | 1.0x | 1.07x | yes |
| util-is | ts-node | 2.02 s (1.93–2.21) | 3.46 s | 260 MB | 0.98x | 1.05x | yes |
| util-is | ts-quickjs | 3.30 s (3.18–3.42) | 3.22 s | 73 MB | 0.6x | 0.64x | yes |
| react-19 | yaml-main | 14.25 s (13.25–14.33) | 45.37 s | 84 MB | 0.89x | 1.0x | yes |
| react-19 | yaml-signal | 12.66 s (11.93–12.81) | 45.12 s | 90 MB | 1.0x | 1.13x | yes |
| react-19 | ts-node | 23.69 s (23.20–24.74) | 27.90 s | 671 MB | 0.53x | 0.6x | yes |
| react-19 | ts-quickjs | 25.61 s (25.42–27.31) | 25.21 s | 87 MB | 0.49x | 0.56x | yes |
| audit | yaml-main | 15.34 s (14.38–16.45) | 38.44 s | 81 MB | 0.96x | 1.0x | yes |
| audit | yaml-signal | 14.66 s (13.84–15.14) | 39.00 s | 82 MB | 1.0x | 1.05x | yes |
| audit | ts-node | 8.98 s (8.68–9.87) | 21.21 s | 711 MB | 1.63x | 1.71x | yes |
| audit | ts-quickjs | 20.55 s (19.98–21.37) | 20.27 s | 144 MB | 0.71x | 0.75x | yes |

### calcom

| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |
|---|---|---:|---:|---:|---:|---:|:-:|
| util-is | yaml-main | 12.24 s (12.22–12.30) | 37.20 s | 219 MB | 0.93x | 1.0x | yes |
| util-is | yaml-signal | 11.39 s (11.24–11.50) | 37.93 s | 219 MB | 1.0x | 1.07x | yes |
| util-is | ts-node | 8.96 s (8.88–9.32) | 12.89 s | 397 MB | 1.27x | 1.37x | yes |
| util-is | ts-quickjs | 12.67 s (12.54–12.93) | 12.49 s | 235 MB | 0.9x | 0.97x | yes |
| debarrel | yaml-main | 33.31 s (33.29–35.31) | 123.14 s | 291 MB | 1.01x | 1.0x | yes |
| debarrel | yaml-signal | 33.54 s (32.81–34.76) | 123.21 s | 290 MB | 1.0x | 0.99x | yes |
| debarrel | ts-node | 70.04 s (68.70–70.11) | 72.55 s | 1244 MB | 0.48x | 0.48x | yes |
| debarrel | ts-quickjs | 66.84 s (66.37–69.98) | 66.18 s | 294 MB | 0.5x | 0.5x | yes |
| audit | yaml-main | 80.38 s (79.37–84.34) | 188.28 s | 64 MB | 0.98x | 1.0x | yes |
| audit | yaml-signal | 78.76 s (78.60–79.32) | 187.76 s | 64 MB | 1.0x | 1.02x | yes |
| audit | ts-node | 23.18 s (22.68–23.49) | 53.88 s | 621 MB | 3.4x | 3.47x | yes |
| audit | ts-quickjs | 56.80 s (55.47–60.06) | 57.20 s | 138 MB | 1.39x | 1.42x | yes |

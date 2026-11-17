"""overview.svg: wall-time speedup of each TS runtime over YAML+signal, one row per (workflow, repo).
Usage: overview.py summary.csv out.svg"""
import csv, math, sys
rows = list(csv.DictReader(open(sys.argv[1])))
cells = {}
for r in rows:
    if r["repo"] == "probe": continue
    cells.setdefault((r["workflow"], r["repo"]), {})[r["arm"]] = r
keys = sorted(cells, key=lambda k: float(cells[k]["ts-node"]["speedup_vs_yaml_signal"]))
W, left, rowh, top = 760, 230, 34, 56
H = top + rowh * len(keys) + 50
cx = left + (W - left - 40) / 2                     # x of 1.0x
span = (W - left - 40) / 2 / math.log2(4)          # log2 scale, 0.25x..4x
x = lambda v: cx + math.log2(v) * span
c = {"ts-node": "#2f80ed", "ts-quickjs": "#27ae60"}
o = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" font-family="system-ui,sans-serif" font-size="12">',
     '<rect width="100%" height="100%" fill="#fff"/>',
     '<text x="16" y="22" font-size="15" font-weight="600" fill="#1f2933">TS engine wall time vs YAML engine (with completion signal)</text>',
     f'<text x="16" y="40" fill="#52606d">Right of the line: TS faster. Median of 5 runs (3 on cal.com), 4 vCPU. Log scale.</text>']
for v in (0.25, 0.5, 1, 2, 4):
    o.append(f'<line x1="{x(v):.1f}" y1="{top-6}" x2="{x(v):.1f}" y2="{H-40}" stroke="{"#1f2933" if v==1 else "#e4e7eb"}"/>')
    o.append(f'<text x="{x(v):.1f}" y="{H-24}" text-anchor="middle" fill="#52606d">{v:g}x</text>')
for i, k in enumerate(keys):
    y = top + i * rowh
    o.append(f'<text x="{left-10}" y="{y+18}" text-anchor="end" fill="#1f2933">{k[0]} · {k[1]}</text>')
    for j, arm in enumerate(("ts-node", "ts-quickjs")):
        v = float(cells[k][arm]["speedup_vs_yaml_signal"])
        x0, x1 = sorted((cx, x(v)))
        yy = y + 5 + j * 12
        o.append(f'<rect x="{x0:.1f}" y="{yy}" width="{max(x1-x0,1):.1f}" height="10" fill="{c[arm]}"/>')
        o.append(f'<text x="{(x1+4) if v>=1 else (x0-4):.1f}" y="{yy+9}" text-anchor="{"start" if v>=1 else "end"}" fill="#52606d" font-size="10">{v:.2f}x</text>')
lx = 16
for arm in c:
    o.append(f'<rect x="{lx}" y="{H-14}" width="10" height="10" fill="{c[arm]}"/><text x="{lx+14}" y="{H-5}" fill="#1f2933">{arm}</text>'); lx += 100
o.append('</svg>')
open(sys.argv[2], "w").write("\n".join(o))

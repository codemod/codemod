#!/usr/bin/env python3
# summarize.py <csv> [label=rename ...]: median wall/cpu/rss per cell and arm, speedup vs mo, diff agreement.
import csv, statistics as st, sys
from collections import defaultdict
rows = list(csv.DictReader(open(sys.argv[1])))
ren = dict(a.split("=", 1) for a in sys.argv[2:])
cells = defaultdict(lambda: defaultdict(list))
for r in rows: cells[(r["workload"], r["repo"])][ren.get(r["arm"], r["arm"])].append(r)
for (wl, repo), arms in cells.items():
    base = st.median(float(r["wall_s"]) for r in arms.get("mo", [])) if "mo" in arms else None
    diffs = {a: {r["diff"] for r in rs} for a, rs in arms.items()}
    agree = len(set().union(*diffs.values())) == 1
    print(f"\n{wl} on {repo}  (diffs {'all identical' if agree else 'DIFFER: ' + str(diffs)})")
    print(f"  {'arm':22} {'wall s':>8} {'vs mo':>6} {'cpu s':>7} {'peak MB':>8} {'n':>2} exit")
    for a, rs in sorted(arms.items(), key=lambda kv: st.median(float(r['wall_s']) for r in kv[1])):
        w = st.median(float(r["wall_s"]) for r in rs)
        c = st.median(float(r["cpu_s"]) for r in rs)
        m = st.median(float(r["rss_mb"]) for r in rs)
        x = f"{base / w:5.2f}x" if base else ""
        print(f"  {a:22} {w:8.2f} {x:>6} {c:7.1f} {m:8.0f} {len(rs):2} {','.join(sorted({r['exit'] for r in rs}))}")

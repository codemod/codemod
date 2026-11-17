#!/usr/bin/env python3
"""Summarise raw.csv from bench.py: medians, spread, speedups, diff checks, SVG chart.

Usage: python3 -I analyze.py results/raw.csv results/
Writes summary.csv, summary.md and chart-<repo>.svg into the output directory.
"""
import csv, os, statistics, sys
from collections import defaultdict

ARMS = ["yaml-main", "yaml-signal", "ts-node", "ts-quickjs"]
COLORS = {"yaml-main": "#9aa5b1", "yaml-signal": "#52606d", "ts-node": "#2f80ed", "ts-quickjs": "#27ae60"}
REPOS = ["probe", "bluebird", "react-bootstrap-v1", "taxonomy", "excalidraw", "calcom"]

def q(xs, p):
    xs = sorted(xs)
    k = (len(xs) - 1) * p
    lo, hi = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)

def main(raw, outdir):
    rows = list(csv.DictReader(open(raw)))
    cells = defaultdict(list)
    for r in rows:
        cells[(r["workflow"], r["repo"], r["arm"])].append(r)
    workflows = list(dict.fromkeys(r["workflow"] for r in rows))
    summary = []
    for wf in workflows:
        for repo in REPOS:
            shas = {}
            for arm in ARMS:
                rs = cells.get((wf, repo, arm))
                if not rs:
                    continue
                walls = [float(r["wall_s"]) for r in rs]
                cpus = [float(r["cpu_s"]) for r in rs]
                rss = [float(r["peak_rss_mb"]) for r in rs]
                ok = all(r["exit"] == "0" for r in rs)
                sha_set = {r["diff_sha"] for r in rs}
                shas[arm] = sha_set
                summary.append({
                    "workflow": wf, "repo": repo, "arm": arm, "n": len(rs),
                    "wall_median_s": statistics.median(walls), "wall_min_s": min(walls), "wall_max_s": max(walls),
                    "wall_iqr_s": q(walls, .75) - q(walls, .25),
                    "cpu_median_s": statistics.median(cpus), "peak_rss_median_mb": statistics.median(rss),
                    "all_exit_0": ok, "diff_sha": "|".join(sorted(sha_set)),
                    "changed_files": rs[0]["changed_files"],
                })
            ref = shas.get("yaml-signal") or shas.get("yaml-main")
            for s in summary:
                if s["workflow"] == wf and s["repo"] == repo:
                    s["diff_matches_yaml"] = (shas.get(s["arm"]) == ref) if ref else ""
    base = {(s["workflow"], s["repo"]): s for s in summary if s["arm"] == "yaml-signal"}
    stock = {(s["workflow"], s["repo"]): s for s in summary if s["arm"] == "yaml-main"}
    for s in summary:
        b, m = base.get((s["workflow"], s["repo"])), stock.get((s["workflow"], s["repo"]))
        s["speedup_vs_yaml_signal"] = round(b["wall_median_s"] / s["wall_median_s"], 2) if b else ""
        s["speedup_vs_yaml_main"] = round(m["wall_median_s"] / s["wall_median_s"], 2) if m else ""
    os.makedirs(outdir, exist_ok=True)
    keys = list(summary[0].keys())
    with open(os.path.join(outdir, "summary.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, keys)
        w.writeheader()
        for s in summary:
            w.writerow({k: (f"{v:.3f}" if isinstance(v, float) else v) for k, v in s.items()})
    # markdown: one table per repo, wall median [min-max], cpu, rss, speedup
    md = []
    for repo in REPOS:
        sub = [s for s in summary if s["repo"] == repo]
        if not sub:
            continue
        md.append(f"\n### {repo}\n")
        md.append("| Workflow | Arm | Wall median (min–max) | CPU | Peak RSS | vs YAML+signal | vs YAML today | Same diff |")
        md.append("|---|---|---:|---:|---:|---:|---:|:-:|")
        for s in sub:
            same = "—" if s["diff_matches_yaml"] == "" else ("yes" if s["diff_matches_yaml"] else "**no**")
            ex = "" if s["all_exit_0"] else " (failed)"
            md.append(f"| {s['workflow']} | {s['arm']}{ex} | {s['wall_median_s']:.2f} s ({s['wall_min_s']:.2f}–{s['wall_max_s']:.2f}) | "
                      f"{s['cpu_median_s']:.2f} s | {s['peak_rss_median_mb']:.0f} MB | {s['speedup_vs_yaml_signal']}x | {s['speedup_vs_yaml_main']}x | {same} |")
    open(os.path.join(outdir, "summary.md"), "w").write("\n".join(md) + "\n")
    for repo in REPOS:
        chart(summary, workflows, repo, os.path.join(outdir, f"chart-{repo}.svg"))

def chart(summary, workflows, repo, path):
    sub = {(s["workflow"], s["arm"]): s for s in summary if s["repo"] == repo}
    wfs = [w for w in workflows if any((w, a) in sub for a in ARMS)]
    if not wfs:
        return
    W, rowh, barh, left = 760, 22 * len(ARMS) + 18, 18, 120
    H = 60 + rowh * len(wfs) + 30
    vmax = max(s["wall_max_s"] for s in sub.values()) * 1.08
    sx = lambda v: left + (W - left - 90) * v / vmax
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" font-family="system-ui, sans-serif" font-size="12">',
           f'<rect width="{W}" height="{H}" fill="white"/>',
           f'<text x="12" y="22" font-size="15" font-weight="600">Wall time on {repo} (median, whiskers = min–max; lower is better)</text>']
    for i, arm in enumerate(ARMS):
        out.append(f'<rect x="{12 + i * 150}" y="34" width="12" height="12" fill="{COLORS[arm]}"/><text x="{28 + i * 150}" y="44">{arm}</text>')
    for j, wf in enumerate(wfs):
        y0 = 60 + j * rowh
        out.append(f'<text x="12" y="{y0 + rowh / 2}" font-weight="600">{wf}</text>')
        for i, arm in enumerate(ARMS):
            s = sub.get((wf, arm))
            if not s:
                continue
            y = y0 + i * 22
            out.append(f'<rect x="{left}" y="{y}" width="{sx(s["wall_median_s"]) - left:.1f}" height="{barh}" fill="{COLORS[arm]}"/>')
            out.append(f'<line x1="{sx(s["wall_min_s"]):.1f}" x2="{sx(s["wall_max_s"]):.1f}" y1="{y + barh / 2}" y2="{y + barh / 2}" stroke="black"/>')
            out.append(f'<text x="{sx(s["wall_max_s"]) + 6:.1f}" y="{y + 13}">{s["wall_median_s"]:.2f} s</text>')
    out.append("</svg>")
    open(path, "w").write("\n".join(out))

if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])

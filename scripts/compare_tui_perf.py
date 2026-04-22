#!/usr/bin/env python3

from __future__ import annotations

import pathlib
import re
import sys


COUNTER_PATTERN = re.compile(r"([a-z_]+)=([0-9]+)")
SCENARIOS = ["completed", "awaiting_trigger", "active"]


def load_counters(directory: pathlib.Path) -> dict[str, dict[str, int]]:
    data: dict[str, dict[str, int]] = {}
    for scenario in SCENARIOS:
        counter_file = directory / f"{scenario}.counters"
        text = counter_file.read_text(encoding="utf-8").strip()
        if not text:
            raise SystemExit(f"missing counter line in {counter_file}")
        counters = {key: int(value) for key, value in COUNTER_PATTERN.findall(text)}
        if not counters:
            raise SystemExit(f"failed to parse counters from {counter_file}: {text}")
        data[scenario] = counters
    return data


def render_delta(delta: int) -> str:
    if delta > 0:
        return f"+{delta}"
    return str(delta)


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: compare_tui_perf.py <baseline_dir> <candidate_dir>")

    baseline_dir = pathlib.Path(sys.argv[1])
    candidate_dir = pathlib.Path(sys.argv[2])
    baseline = load_counters(baseline_dir)
    candidate = load_counters(candidate_dir)

    lines = [
        "# TUI Perf Comparison",
        "",
        f"- Baseline dir: `{baseline_dir}`",
        f"- Candidate dir: `{candidate_dir}`",
        "",
    ]

    for scenario in SCENARIOS:
        lines.extend(
            [
                f"## {scenario.replace('_', ' ').title()}",
                "",
                "| Metric | Baseline | Candidate | Delta |",
                "| --- | ---: | ---: | ---: |",
            ]
        )
        metrics = sorted(set(baseline[scenario]) | set(candidate[scenario]))
        for metric in metrics:
            before = baseline[scenario].get(metric, 0)
            after = candidate[scenario].get(metric, 0)
            lines.append(
                f"| `{metric}` | {before} | {after} | {render_delta(after - before)} |"
            )
        lines.append("")

    sys.stdout.write("\n".join(lines))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

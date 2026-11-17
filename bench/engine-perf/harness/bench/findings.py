"""Compare audit findings: YAML 'Metrics:' block vs TS JSON output. Usage: findings.py <yaml.log> <ts.log>"""
import json, re, sys
from collections import Counter

def yaml_counts(path):
    text = open(path, encoding="utf-8", errors="replace").read()
    text = re.sub(r"\x1b\[[0-9;]*m", "", text)
    c, metric = Counter(), None
    if "Metrics:" not in text:
        return c
    for line in text.split("Metrics:", 1)[1].splitlines():
        if re.match(r"^  \S.*:$", line):
            metric = line.strip()[:-1]
        elif m := re.match(r"^    (.*): (\d+)$", line):
            dims = tuple(sorted(p.strip() for p in m.group(1).split(",") if p.strip()))
            c[(metric, dims)] += int(m.group(2))
    return c

def ts_counts(path):
    text = open(path, encoding="utf-8", errors="replace").read()
    data = json.loads(text[text.index("["):])
    c = Counter()
    def walk(x):
        if isinstance(x, list):
            for y in x: walk(y)
        elif isinstance(x, dict) and "metric" in x:
            c[(x["metric"], tuple(sorted(f"{k}={v}" for k, v in x["dims"].items())))] += 1
    walk(data)
    return c

y, t = yaml_counts(sys.argv[1]), ts_counts(sys.argv[2])
print(f"yaml total={sum(y.values())} ts total={sum(t.values())} identical={y == t}")
for k in sorted(set(y) | set(t)):
    if y[k] != t[k]: print("  diff", k, y[k], t[k])

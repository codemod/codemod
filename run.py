#!/usr/bin/env python3
# Runs each cell: 1 warmup + N measured rounds, arms shuffled per round, repo reset before each run.
# Records wall, CPU (children user+sys), peak RSS of the largest process (wait4), exit, diff hash.
import csv, hashlib, os, random, subprocess, sys, time
sys.path.insert(0, os.path.dirname(__file__)); import cells
def sh(a, cwd): return subprocess.run(a, cwd=cwd, check=True, capture_output=True)
def reset(r): sh(["git","reset","-q","--hard"], r); sh(["git","clean","-q","-fdx"], r)
def diff(r):
    sh(["git","add","-A"], r); d = sh(["git","diff","--cached","--binary"], r).stdout
    return hashlib.sha256(d).hexdigest()[:12], len(sh(["git","diff","--cached","--name-only"], r).stdout.split())
def once(cmd, log):
    t0 = time.perf_counter()
    with open(log, "wb") as f:
        p = subprocess.Popen(cmd, stdout=f, stderr=subprocess.STDOUT, cwd="/tmp", start_new_session=True, env=dict(os.environ, NO_COLOR="1"))
        _, st, ru = os.wait4(p.pid, 0)
    return time.perf_counter()-t0, ru.ru_utime+ru.ru_stime, ru.ru_maxrss/1024, os.waitstatus_to_exitcode(st)
group, out = sys.argv[1], sys.argv[2]
logs = os.path.join(os.path.dirname(out), "logs"); os.makedirs(logs, exist_ok=True)
new = not os.path.exists(out); fh = open(out, "a", newline=""); w = csv.writer(fh)
if new: w.writerow(["workload","repo","arm","round","wall_s","cpu_s","rss_mb","exit","diff","files"])
rng = random.Random(7)
for wl, repo, n, arms in getattr(cells, group):
    rp = os.path.join(cells.R, repo)
    for rnd in range(-1, n):
        order = list(arms); rng.shuffle(order)
        for arm in order:
            reset(rp)
            wall, cpu, rss, code = once(arms[arm] + ["-t", rp], f"{logs}/{wl}.{repo}.{arm}.{rnd}.log")
            h, nf = diff(rp); reset(rp)
            row = [wl, repo, arm, rnd, f"{wall:.3f}", f"{cpu:.2f}", f"{rss:.0f}", code, h, nf]
            print(*row, flush=True)
            if rnd >= 0: w.writerow(row); fh.flush()

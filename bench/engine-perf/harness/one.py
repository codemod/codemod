#!/usr/bin/env python3
# one.py <repo> <cmd...>: reset repo, run cmd -t repo, print wall, cpu, peak RSS (MB), exit, diff hash, files; reset.
import hashlib, os, subprocess, sys, time
repo, cmd = os.path.abspath(sys.argv[1]), sys.argv[2:]
def sh(a): return subprocess.run(a, cwd=repo, check=True, capture_output=True)
def reset(): sh(["git", "reset", "-q", "--hard"]); sh(["git", "clean", "-q", "-fdx"])
reset()
t0 = time.perf_counter()
with open(os.environ.get("LOG", "/tmp/one.log"), "wb") as f:
    p = subprocess.Popen(cmd + ["-t", repo], stdout=f, stderr=subprocess.STDOUT, cwd="/tmp", env=dict(os.environ, NO_COLOR="1"))
    _, st, ru = os.wait4(p.pid, 0)
wall = time.perf_counter() - t0
sh(["git", "add", "-A"])
d = sh(["git", "diff", "--cached", "--binary"]).stdout
n = len(sh(["git", "diff", "--cached", "--name-only"]).stdout.split())
reset()
print(f"wall={wall:.2f}s cpu={ru.ru_utime+ru.ru_stime:.1f}s rss={ru.ru_maxrss/1024:.0f}MB exit={os.waitstatus_to_exitcode(st)} diff={hashlib.sha256(d).hexdigest()[:12]} files={n}", flush=True)

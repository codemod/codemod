#!/usr/bin/env python3
"""YAML engine vs TS orchestration runtimes on real workflows and repos.

For every (workflow, repo) cell: one warmup round, then N measured rounds.
Each round runs every arm once, in a shuffled order, on a freshly reset
checkout. Per run it records wall time, CPU (user+sys of the process tree),
peak RSS (largest single process in the tree, from wait4), the exit code,
and a hash of the staged diff (`git add -A; git diff --cached --binary`).

Usage: python3 -I bench.py --config config.json --out results/raw.csv [--runs 5]
The repos are untrusted data: nothing inside them is executed; they are only
passed as targets and reset/cleaned with git.
"""
import argparse, csv, hashlib, json, os, random, shlex, subprocess, sys, time

def sh(args, cwd, **kw):
    return subprocess.run(args, cwd=cwd, check=True, capture_output=True, **kw)

def reset(repo):
    sh(["git", "reset", "-q", "--hard"], repo)
    sh(["git", "clean", "-q", "-fdx"], repo)

def diff_state(repo):
    sh(["git", "add", "-A"], repo)
    diff = sh(["git", "diff", "--cached", "--binary"], repo).stdout
    files = sh(["git", "diff", "--cached", "--name-only"], repo).stdout.decode().split()
    return hashlib.sha256(diff).hexdigest()[:16], len(files)

def run_once(cmd, env, log_path, timeout):
    t0 = time.perf_counter()
    with open(log_path, "wb") as log:
        proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, env=env,
                                cwd="/tmp", start_new_session=True)
        deadline = t0 + timeout
        while True:
            pid, status, ru = os.wait4(proc.pid, os.WNOHANG)
            if pid:
                break
            if time.perf_counter() > deadline:
                os.killpg(proc.pid, 9)
                pid, status, ru = os.wait4(proc.pid, 0)
                break
            time.sleep(0.005)
    wall = time.perf_counter() - t0
    code = os.waitstatus_to_exitcode(status)
    return wall, ru.ru_utime + ru.ru_stime, ru.ru_maxrss / 1024.0, code

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--warmup", type=int, default=1)
    ap.add_argument("--only-workflows", default="")
    ap.add_argument("--only-repos", default="")
    ap.add_argument("--only-arms", default="")
    ap.add_argument("--timeout", type=float, default=1800)
    a = ap.parse_args()
    cfg = json.load(open(a.config))
    pick = lambda xs, f: [x for x in xs if not f or x["name"] in f.split(",")]
    workflows, repos, arms = pick(cfg["workflows"], a.only_workflows), pick(cfg["repos"], a.only_repos), pick(cfg["arms"], a.only_arms)
    logdir = os.path.join(os.path.dirname(os.path.abspath(a.out)), "logs")
    os.makedirs(logdir, exist_ok=True)
    new = not os.path.exists(a.out)
    out = open(a.out, "a", newline="")
    w = csv.writer(out)
    if new:
        w.writerow(["workflow", "repo", "arm", "round", "wall_s", "cpu_s", "peak_rss_mb", "exit", "diff_sha", "changed_files", "ts"])
    rng = random.Random(42)
    for wf in workflows:
        for repo in repos:
            for rnd in range(-a.warmup, a.runs):
                order = list(arms)
                rng.shuffle(order)
                for arm in order:
                    if wf["name"] in arm.get("skip_workflows", []):
                        continue
                    reset(repo["path"])
                    cmd = [c.format(yaml=os.path.join(wf["dir"], "yaml", "workflow.yaml"),
                                    ts=os.path.join(wf["dir"], "ts", "workflow.ts"),
                                    repo=repo["path"], **cfg.get("vars", {}))
                           for c in arm["cmd"] + wf.get("extra_args", {}).get(arm["engine"], [])]
                    env = dict(os.environ, **cfg.get("env", {}), **arm.get("env", {}))
                    log = os.path.join(logdir, f"{wf['name']}.{repo['name']}.{arm['name']}.{'warm' if rnd < 0 else rnd}.log")
                    wall, cpu, rss, code = run_once(cmd, env, log, a.timeout)
                    sha, nfiles = diff_state(repo["path"])
                    row = [wf["name"], repo["name"], arm["name"], rnd, f"{wall:.3f}", f"{cpu:.3f}", f"{rss:.1f}", code, sha, nfiles, int(time.time())]
                    if rnd >= 0:
                        w.writerow(row); out.flush()
                    print(" ".join(map(str, row)), flush=True)
                    reset(repo["path"])

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Grade one codeimage checkout against the core-Solid finish line.

Usage: grade.py <checkout> [--baseline baseline.json] [--write-baseline out.json] [--extras]
Finish line: every Solid package declares solid-js 2.x, and tsc in each Solid package
shows no errors beyond the Solid 1 baseline except ones traced to libraries that have
no Solid 2 release (left alone by design).
"""
import json, os, re, subprocess, sys

PKGS = ["apps/codeimage", "apps/website", "packages/ui", "packages/atomic-state",
        "packages/dom-export", "packages/highlight", "packages/locale", "packages/config"]
BLOCKED = ["@kobalte/core", "@kobalte/utils", "@kobalte/vanilla-extract", "@motionone/solid",
           "@thisbeyond/solid-dnd", "solid-codemirror", "solid-headless", "solid-mdx",
           "solid-toast", "@solid-aria/types", "@solid-aria/utils", "solid-motionone",
           # also have no Solid 2 release but were missed in the agents' prompt list
           "statebuilder", "@codeui/kit", "@corvu/utils"]
ERR = re.compile(r"^(?P<file>[^\s(][^(]*)\((?P<line>\d+),(?P<col>\d+)\): error (?P<code>TS\d+): (?P<msg>.*)$")
IMPORT = re.compile(r"import\s+(?:type\s+)?(?P<names>[\s\S]*?)\s+from\s+['\"](?P<mod>[^'\"]+)['\"]")

def sh(cmd, cwd, timeout=1200):
    p = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout + p.stderr

def is_blocked(mod):
    return any(mod == b or mod.startswith(b + "/") for b in BLOCKED)

def blocked_names(path, cache={}):
    if path in cache: return cache[path]
    names = set()
    try: src = open(path, encoding="utf8").read()
    except OSError: src = ""
    for m in IMPORT.finditer(src):
        if is_blocked(m["mod"]):
            for n in re.findall(r"[A-Za-z_$][\w$]*", m["names"]):
                if n not in ("as", "type"): names.add(n)
    cache[path] = names
    return names

def classify(root, pkg, e):
    if any(b in e["msg"] for b in BLOCKED): return "library"
    path = os.path.join(root, pkg, e["file"])
    names = blocked_names(path)
    if names:
        try: line = open(path, encoding="utf8").read().split("\n")[int(e["line"]) - 1]
        except (OSError, IndexError): line = ""
        if any(re.search(r"(?<![\w$])" + re.escape(n) + r"(?![\w$])", line + " " + e["msg"]) for n in names):
            return "library"
    return "core"

def solid_ranges(root):
    cat = {}
    ws = open(os.path.join(root, "pnpm-workspace.yaml")).read()
    m = re.search(r"^\s+solid-js:\s*['\"]?([^'\"\n]+)", ws, re.M)
    cat_solid = m.group(1).strip() if m else None
    out = {}
    for pkg in PKGS:
        d = json.load(open(os.path.join(root, pkg, "package.json")))
        for k in ("dependencies", "devDependencies", "peerDependencies"):
            v = d.get(k, {}).get("solid-js")
            if v is None: continue
            if v.startswith("catalog:"): v = cat_solid
            out[f"{pkg} {k}"] = v
    return out

def is_v2(r):
    return bool(r) and bool(re.match(r"^[\^~>=\s]*2\.", r)) or (r or "").startswith("2.")

def key(pkg, e):
    return f"{pkg}/{e['file']}|{e['code']}|{e['msg']}"

def main():
    root = os.path.abspath(sys.argv[1]); args = sys.argv[2:]
    base = set()
    if "--baseline" in args: base = set(json.load(open(args[args.index("--baseline") + 1])))
    res = {"checkout": root}
    res["install_exit"], _ = sh("CI=1 HUSKY=0 pnpm install --no-frozen-lockfile", root)
    res["libs_build_exit"], _ = sh("pnpm libs:build", root)
    ranges = solid_ranges(root)
    res["solid_ranges"] = ranges
    res["all_solid_2"] = all(is_v2(v) for v in ranges.values())
    errs, keys = [], []
    for pkg in PKGS:
        _, out = sh("pnpm exec tsc --noEmit -p .", os.path.join(root, pkg))
        for ln in out.splitlines():
            m = ERR.match(ln.strip())
            if not m: continue
            e = m.groupdict(); e["pkg"] = pkg
            k = key(pkg, e); keys.append(k)
            e["kind"] = "baseline" if k in base else classify(root, pkg, e)
            errs.append(e)
    count = lambda kind: sum(1 for e in errs if e["kind"] == kind)
    res["tsc"] = {"total": len(errs), "baseline": count("baseline"),
                  "library": count("library"), "core": count("core")}
    res["core_errors"] = [f"{e['pkg']}/{e['file']}:{e['line']} {e['code']} {e['msg'][:160]}"
                          for e in errs if e["kind"] == "core"]
    res["finished"] = res["all_solid_2"] and res["tsc"]["core"] == 0
    if "--extras" in args:
        res["build_app_exit"], _ = sh("pnpm build", os.path.join(root, "apps/codeimage"))
        res["build_website_exit"], _ = sh("pnpm build", os.path.join(root, "apps/website"))
        _, out = sh("pnpm test", os.path.join(root, "packages/dom-export"))
        m = re.search(r"Tests\s+(.*)", out); res["dom_export_tests"] = m.group(1).strip() if m else "no result"
    _, stat = sh("git add -A -N . && git diff --shortstat -- . ':!pnpm-lock.yaml' && git reset -q", root)
    res["diff"] = stat.strip()
    if "--write-baseline" in args:
        json.dump(sorted(set(keys)), open(args[args.index("--write-baseline") + 1], "w"), indent=1)
    print(json.dumps(res, indent=1))

main()

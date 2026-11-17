#!/usr/bin/env python3
"""Count non-test, non-comment, non-blank lines per engine and category.

Usage: count.py <yaml_root=/home/user/codemod> <ts_root=/home/user/ts-orch> <out_csv>

Method:
- Rust/TS/JS: a small lexer removes comments (//, /* */ incl. nested Rust blocks, doc comments)
  and blanks out string contents, so braces inside strings or comments never count.
  A line counts when anything but whitespace remains.
- Rust: every item annotated #[cfg(test)] / #[cfg(all(test, ..))] (usually `mod tests { .. }`)
  is removed by brace matching on the comment- and string-free text; its lines are reported
  as test_lines_excluded. Files under tests/ are not listed at all.
- JSON: non-blank lines. HTML: non-blank lines outside <!-- --> (includes inline CSS/JS).
- Ranges (1-based, inclusive) split a file by function; every range is listed in the manifest.
"""
import csv, os, re, sys

YAML = sys.argv[1] if len(sys.argv) > 1 else "/home/user/codemod"
TS = sys.argv[2] if len(sys.argv) > 2 else "/home/user/ts-orch"
OUT = sys.argv[3] if len(sys.argv) > 3 else "engine-size.csv"


def strip_code(text, rust):
    """Return text with comments removed and string contents replaced by spaces; newlines kept."""
    out = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            while i < n and text[i] != "\n":
                i += 1
            continue
        if c == "/" and nxt == "*":
            depth = 1
            i += 2
            while i < n and depth:
                if rust and text[i] == "/" and i + 1 < n and text[i + 1] == "*":
                    depth += 1
                    i += 2
                    continue
                if text[i] == "*" and i + 1 < n and text[i + 1] == "/":
                    depth -= 1
                    i += 2
                    continue
                if text[i] == "\n":
                    out.append("\n")
                i += 1
            continue
        if rust and c == "r" and (nxt == '"' or nxt == "#") and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                end = '"' + "#" * hashes
                k = text.find(end, j + 1)
                k = n if k < 0 else k
                seg = text[i : k + len(end)]
                out.append("S" + re.sub(r"[^\n]", " ", seg[1:]))
                i = k + len(end)
                continue
        if c == '"' or (not rust and c in "'`"):
            q = c
            out.append("S")
            i += 1
            while i < n and text[i] != q:
                if text[i] == "\\":
                    out.append("  " if text[i + 1 : i + 2] != "\n" else " \n")
                    i += 2
                    continue
                out.append("\n" if text[i] == "\n" else " ")
                i += 1
            out.append(" ")
            i += 1
            continue
        if rust and c == "'":
            # char literal ('x', '\n', '\u{..}') vs lifetime ('a)
            m = re.match(r"'(\\.[^']*|[^\\'\n])'", text[i : i + 12])
            if m:
                out.append("C" + " " * (len(m.group(0)) - 1))
                i += len(m.group(0))
                continue
        out.append(c)
        i += 1
    return "".join(out)


CFG_TEST = re.compile(r"#\[cfg\((?:all\()?\s*test\b")


def test_regions(code):
    """Line spans (0-based, inclusive) of #[cfg(test)] items in comment/string-free code."""
    spans = []
    for m in CFG_TEST.finditer(code):
        start = m.start()
        j = code.find("]", m.end())
        k = j + 1
        brace = code.find("{", k)
        semi = code.find(";", k)
        if brace < 0 or (0 <= semi < brace):
            end = semi
        else:
            depth = 0
            end = brace
            for p in range(brace, len(code)):
                if code[p] == "{":
                    depth += 1
                elif code[p] == "}":
                    depth -= 1
                    if depth == 0:
                        end = p
                        break
        spans.append((code.count("\n", 0, start), code.count("\n", 0, end)))
    return spans


def count_file(path, ranges=None):
    with open(path, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
    ext = os.path.splitext(path)[1]
    lines_total = text.count("\n") + 1
    if ext == ".json":
        flags = [bool(l.strip()) for l in text.split("\n")]
        tests = set()
    elif ext == ".html":
        no_comments = re.sub(r"<!--.*?-->", lambda m: re.sub(r"[^\n]", " ", m.group(0)), text, flags=re.S)
        flags = [bool(l.strip()) for l in no_comments.split("\n")]
        tests = set()
    else:
        rust = ext == ".rs"
        code = strip_code(text, rust)
        flags = [bool(l.strip()) for l in code.split("\n")]
        tests = set()
        if rust:
            for a, b in test_regions(code):
                tests.update(range(a, b + 1))
    sel = range(len(flags)) if not ranges else [i for a, b in ranges for i in range(a - 1, min(b, len(flags)))]
    code_lines = sum(1 for i in sel if flags[i] and i not in tests)
    test_lines = sum(1 for i in sel if flags[i] and i in tests)
    return code_lines, test_lines, lines_total


# ---------------------------------------------------------------------------
# Manifest: (engine, category, feature, path relative to repo root, ranges or None, note)
# categories: a-core | a-step-exec | b-jssg-glue | c-yaml-only | c-ts-only | excluded
# ---------------------------------------------------------------------------
Y = "yaml"
T = "ts"
M = []


def y(cat, feat, path, ranges=None, note=""):
    M.append((Y, cat, feat, path, ranges, note))


def t(cat, feat, path, ranges=None, note=""):
    M.append((T, cat, feat, path, ranges, note))


# ---- YAML engine (codemod main) ----
for f in ["error", "lib", "node", "runtime", "schema", "state_diff", "step", "strategy", "task", "template", "trigger", "variable", "workflow"]:
    y("a-core", "workflow model + templating", f"crates/models/src/{f}.rs")
y("a-core", "scheduler (DAG/matrix)", "crates/scheduler/src/lib.rs")
y("a-core", "state (local)", "crates/state/src/lib.rs")
y("a-core", "state (local)", "crates/state/src/local_adapter.rs")
y("c-yaml-only", "cloud state adapter", "crates/state/src/cloud_adapter.rs")
y("excluded", "test support", "crates/state/src/mock_adapter.rs", note="pub mock adapter used by tests")
y("a-step-exec", "shell step", "crates/runners/src/lib.rs")
y("a-step-exec", "shell step", "crates/runners/src/direct_runner.rs")
y("c-yaml-only", "Windows cmd runner", "crates/runners/src/cmd_script.rs")
y("c-yaml-only", "container runners (docker/podman)", "crates/runners/src/docker_runner.rs")
y("c-yaml-only", "container runners (docker/podman)", "crates/runners/src/podman_runner.rs")

C = "crates/core/src/"
for f in ["lib", "error", "config", "step_executor", "task_state_service", "utils", "progress_output"]:
    y("a-core", "engine core", C + f + ".rs")
y("a-core", "engine core", C + "workflow_runtime.rs", [(1, 189)], "events, commands, event sinks")
y("c-yaml-only", "interactive approvals (shell/PR/capabilities/dirty-git/agent/selection prompts)", C + "workflow_runtime.rs", [(190, 528)])
y("a-core", "engine core", C + "workflow_runtime.rs", [(529, 1149)], "WorkflowSession handle (dispatch, snapshot)")
E = C + "engine.rs"
y("a-core", "engine core", E, [(1, 134), (147, 168), (192, 229), (578, 633), (1071, 1695), (1865, 1977), (1999, 2803), (4151, 4319), (4345, 5248)],
  "struct/ctor, task spawn, run/cancel/status/list, execute_workflow, recompile_matrix_tasks, execute_task, prepare/finalize step")
y("c-yaml-only", "git ops / managed git / PRs", E, [(135, 146), (315, 319), (1036, 1041)])
y("c-yaml-only", "install-skill step", E, [(169, 191)])
y("b-jssg-glue", "JSSG step glue", E, [(230, 303), (320, 493), (536, 577), (3087, 3131)], "progress units, idle timeout, glob lists, execute_js_ast_grep_step")
y("c-yaml-only", "sharding", E, [(304, 314), (3673, 4022)])
y("c-yaml-only", "registry + nested codemod steps", E, [(494, 521), (1978, 1998), (3447, 3672), (4320, 4344)])
y("c-yaml-only", "interactive approvals (shell/PR/capabilities/dirty-git/agent/selection prompts)", E, [(522, 535)])
y("a-step-exec", "agent/AI step", E, [(634, 653), (741, 1035), (3132, 3446)], "record_llm_result, agent candidates, launch_agent, execute_ai_step")
y("c-yaml-only", "capabilities (LLRT module permissions)", E, [(654, 740)])
y("c-yaml-only", "AI handoff to parent coding agent", E, [(1042, 1070)])
y("c-yaml-only", "resume", E, [(1696, 1765)])
y("c-yaml-only", "manual nodes / triggers", E, [(1766, 1864)])
y("c-yaml-only", "ast-grep YAML-rule step", E, [(2804, 3086)])
y("a-step-exec", "shell step", E, [(4023, 4150)], "execute_run_script_step")
for f in ["execution", "file_ops", "execution_stats", "jssg_execution_service"]:
    y("b-jssg-glue", "JSSG step glue", C + f + ".rs")
y("a-step-exec", "agent/AI step", C + "ai_agent_stream.rs", note="Claude/Codex/OpenCode stream normalizers")
y("a-step-exec", "agent/AI step", C + "llm_usage.rs")
y("a-step-exec", "agent/AI step", C + "ai_handoff.rs", [(1, 332)], "installed-agent discovery + command building")
y("c-yaml-only", "AI handoff to parent coding agent", C + "ai_handoff.rs", [(333, 99999)], "parent coding-agent detection")
y("c-yaml-only", "dry-run diff output", C + "diff.rs")
y("c-yaml-only", "git ops / managed git / PRs", C + "git_ops.rs")
y("c-yaml-only", "git ops / managed git / PRs", C + "managed_git_service.rs")
y("c-yaml-only", "registry + nested codemod steps", C + "nested_codemod_run.rs")
y("c-yaml-only", "registry + nested codemod steps", C + "nested_codemod_service.rs")
y("c-yaml-only", "registry + nested codemod steps", C + "registry.rs")
y("c-yaml-only", "registry + nested codemod steps", C + "registry_link.rs")
y("c-yaml-only", "reports / metrics", C + "report.rs")
y("c-yaml-only", "sharding", C + "shard.rs")
y("c-yaml-only", "structured logging (text/jsonl formats, stdout capture)", C + "structured_log.rs", note="partly core logging; TS has events + plain stdout only")

L = "crates/cli/src/"
y("a-core", "CLI: workflow run/validate", L + "workflow_runner.rs", [(44, 365), (423, 630)], "run + wait loop, task summary, workflow source resolution")
y("c-yaml-only", "manual nodes / triggers", L + "workflow_runner.rs", [(1, 43)])
y("a-core", "CLI: workflow run/validate", L + "commands/workflow/mod.rs")
y("a-core", "CLI: workflow run/validate", L + "commands/workflow/run.rs")
y("a-core", "CLI: workflow run/validate", L + "commands/workflow/validate.rs", note="TS equivalent is tsc --noEmit")
y("a-core", "CLI: workflow run/validate", L + "engine.rs", [(1, 300)], "create_engine, progress/diff callbacks")
y("c-yaml-only", "registry + nested codemod steps", L + "engine.rs", [(301, 99999)], "registry client")
y("c-yaml-only", "resume", L + "commands/workflow/resume.rs")
for f in ["cancel", "list", "status"]:
    y("c-yaml-only", "run management (status/list/cancel)", L + f"commands/workflow/{f}.rs")
y("c-yaml-only", "TUI", L + "commands/workflow/tui.rs")
for f in ["app", "event", "mod", "screens"]:
    y("c-yaml-only", "TUI", L + f"tui/{f}.rs")
y("c-yaml-only", "capabilities (LLRT module permissions)", L + "capabilities_security_callback.rs")
y("c-yaml-only", "git ops / managed git / PRs", L + "dirty_git_check.rs")
y("c-yaml-only", "reports / metrics", L + "report_server.rs")
y("c-yaml-only", "progress bar UI", L + "progress_bar.rs")
y("c-yaml-only", "AI agent UX (log renderer, agent picker)", L + "agent_log_renderer.rs")
y("c-yaml-only", "AI agent UX (log renderer, agent picker)", L + "agent_select.rs")
y("c-yaml-only", "dry-run diff output", L + "pro_dry_run.rs")
y("c-yaml-only", "registry + nested codemod steps", L + "commands/run.rs", note="`codemod run <package>` front door")
y("c-yaml-only", "registry + nested codemod steps", L + "commands/run/llm_usage.rs")

# ---- TS engine (prototype/typescript-orchestration @ 72865ebd) ----
O = "packages/orchestration/"
for f in ["command", "composition", "context", "dynamic", "runnable", "schema", "target"]:
    t("a-core", "authoring model", O + f"src/authoring/{f}.ts")
for f in ["async-context", "errors", "events", "history", "host", "json", "paths", "posix", "protocol"]:
    t("a-core", "core contracts (protocol, history, host seam)", O + f"src/core/{f}.ts")
for f in ["capabilities", "gate", "run"]:
    t("a-core", "run/replay", O + f"src/runtime/{f}.ts")
t("a-core", "scheduler (admission)", O + "src/execution/scheduler.ts")
t("a-core", "run/replay", O + "src/execution/operation-executor.ts")
for f in ["cli", "node-host"]:
    t("a-core", "CLI + Node host", O + f"src/host/{f}.ts")
for f in ["registry", "types", "node", "node-executor"]:
    t("a-core", "runtime registry + node provider", O + f"src/host/runtimes/{f}.ts")
t("a-core", "CLI + Node host", O + "src/index.ts")
t("a-core", "CLI + Node host", O + "src/portable.ts")
t("a-core", "CLI + Node host", O + "bin/codemod-workflow.mjs")
t("a-core", "CLI + Node host", O + "bin/register-ts.mjs", note="in-process type stripping + cache")
for f in ["shell", "process-tree"]:
    t("a-step-exec", "shell step", O + f"src/execution/{f}.ts")
for f in ["agent", "agent-env"]:
    t("a-step-exec", "agent/AI step", O + f"src/execution/{f}.ts")
t("a-step-exec", "agent/AI step", O + "src/host/runtimes/agent-bin.ts")
for f in ["lib", "main", "agent", "external"]:
    t("a-step-exec", "agent/AI step", f"crates/orchestration-agent/src/{f}.rs")
t("b-jssg-glue", "JSSG step glue", O + "src/authoring/transform.ts", note="type-only transform signatures")
for f in ["jssg.ts", "jssg-batch.ts", "files.ts", "languages.json"]:
    t("b-jssg-glue", "JSSG step glue", O + f"src/execution/{f}")
t("b-jssg-glue", "JSSG step glue", O + "src/host/runtimes/node-jssg.ts")
t("b-jssg-glue", "inline-transform bundler (on every run's path)", O + "src/bundle/build.ts")
t("b-jssg-glue", "inline-transform bundler (on every run's path)", O + "src/bundle/cache.ts")
t("b-jssg-glue", "JSSG batch rules (Rust)", "crates/jssg-batch/src/lib.rs")
for f in ["lib", "batch"]:
    t("b-jssg-glue", "napi addon (Rust)", f"crates/jssg-napi/src/{f}.rs")
t("b-jssg-glue", "napi addon (Rust)", "crates/jssg-napi/build.rs")
for f in ["ast-grep.js", "ast-grep.d.ts", "index.js", "index.d.ts", "loader.js", "loader.d.ts", "scripts/build.mjs"]:
    t("b-jssg-glue", "napi addon (JS package)", f"packages/jssg-napi/{f}")
for f in ["lib", "errors", "node_ignore"]:
    t("b-jssg-glue", "native walker (Rust)", f"crates/orchestration-walker/src/{f}.rs")
for f in ["async-context", "dispatch", "entry", "host", "native"]:
    t("c-ts-only", "quickjs runtime (sandboxed alternative)", O + f"src/host/quickjs/{f}.ts")
t("c-ts-only", "quickjs runtime (sandboxed alternative)", O + "src/host/quickjs-bundle.ts")
t("c-ts-only", "quickjs runtime (sandboxed alternative)", O + "src/host/runtimes/quickjs.ts")
for f in ["lib", "main", "modules", "utf8", "select", "jssg", "native"]:
    t("c-ts-only", "quickjs runtime (sandboxed alternative)", f"crates/orchestration-quickjs-host/src/{f}.rs")
t("c-ts-only", "assessment step (TypeSafe)", O + "src/core/assessment.ts")
t("c-ts-only", "assessment step (TypeSafe)", O + "src/execution/assessment.ts")
t("c-ts-only", "assessment step (TypeSafe)", O + "src/host/quickjs/fetch.ts")
t("c-ts-only", "assessment step (TypeSafe)", "crates/orchestration-quickjs-host/src/http.rs")
for f in ["api.ts", "index.ts", "monitor.ts", "server.ts", "session.ts", "ui.html"]:
    t("c-ts-only", "local dashboard", O + f"src/host/dashboard/{f}")
t("c-ts-only", "benchmark timings", O + "src/core/timings.ts")
t("c-ts-only", "benchmark timings", O + "src/host/timings-file.ts")
t("c-ts-only", "benchmark timings", O + "src/execution/child-timings.ts")
for f in ["lib", "on", "off", "report"]:
    t("c-ts-only", "benchmark timings", f"crates/bench-timings/src/{f}.rs")
t("excluded", "test support", O + "src/host/harness.ts", note="./harness export: scripted completions for tests")


def main():
    rows = []
    for eng, cat, feat, rel, ranges, note in M:
        root = YAML if eng == Y else TS
        path = os.path.join(root, rel)
        code, tests, total = count_file(path, ranges)
        rng = "" if not ranges else ";".join(f"{a}-{b if b < 99999 else 'end'}" for a, b in ranges)
        rows.append([eng, cat, feat, rel, rng, code, tests, note])
    with open(OUT, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["engine", "category", "feature", "path", "line_ranges", "code_lines", "test_lines_excluded", "note"])
        w.writerows(rows)

    # unassigned check
    listed = {(e, p) for e, _, _, p, _, _ in M}
    scan = {
        Y: ["crates/models/src", "crates/scheduler/src", "crates/state/src", "crates/runners/src", "crates/core/src"],
        T: [O + "src", O + "bin", "crates/jssg-batch/src", "crates/jssg-napi/src", "crates/orchestration-walker/src",
            "crates/orchestration-agent/src", "crates/orchestration-quickjs-host/src", "crates/bench-timings/src"],
    }
    for eng, dirs in scan.items():
        root = YAML if eng == Y else TS
        for d in dirs:
            for dp, _, fs in os.walk(os.path.join(root, d)):
                for f in fs:
                    rel = os.path.relpath(os.path.join(dp, f), root)
                    if (eng, rel) not in listed:
                        print("UNASSIGNED", eng, rel, file=sys.stderr)

    # summary
    agg = {}
    for eng, cat, feat, *_rest in rows:
        code = _rest[2]
        agg.setdefault((eng, cat), 0)
        agg[(eng, cat)] += code
    for k in sorted(agg):
        print(*k, agg[k], sep="\t")
    feats = {}
    for eng, cat, feat, rel, rng, code, tests, note in rows:
        feats.setdefault((eng, cat, feat), [0, 0])
        feats[(eng, cat, feat)][0] += code
        feats[(eng, cat, feat)][1] += tests
    print()
    for k in sorted(feats):
        print(*k, *feats[k], sep="\t")


if __name__ == "__main__":
    main()

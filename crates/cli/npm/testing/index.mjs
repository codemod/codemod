// `codemod/testing`: run workflows from any test runner (node:test, Vitest,
// Jest, Bun) and compare the files they leave.
//
// Each run copies its target into a temporary directory first. TypeScript
// workflows run in this process (`codemod/host`), so breakpoints, coverage
// and module mocks work in the workflow's code. With `host: "quickjs"`, or
// for YAML workflows, the run goes to the installed codemod CLI instead
// (`codemod workflow harness`), in the engine's own sandboxed runtime.
// Failures are `AssertionError`s with `actual` and `expected`, which every
// runner knows how to show.

import { spawn } from "node:child_process";
import { AssertionError } from "node:assert";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { basename, dirname, extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

// ------------------------------------------------------------ running ---

/**
 * Run a workflow on a copy of `target` and return what happened.
 * Resolves for every outcome (check `run.status`); rejects only when the
 * workflow cannot be run at all (it does not build, the CLI is missing) or
 * an `until` / `given` label matches no step.
 */
export async function runWorkflow(workflow, options) {
  if (!options || options.target === undefined) {
    throw new TypeError("runWorkflow(workflow, { target }): target is required");
  }
  const dir = mkdtempSync(join(tmpdir(), "codemod-test-"));
  try {
    materialize(options.target, dir);
    const before = readTree(dir);
    const result = (await inProcess(workflow, options))
      ? await runInProcess(workflow, options, dir)
      : await harness(
          {
            workflow: toPath(workflow),
            workflowName: options.workflowName,
            target: dir,
            params: options.params ?? {},
            until: options.until,
            given: options.given ?? {},
            approve: options.approve ?? true,
            capabilities: options.capabilities ?? [],
          },
          options.signal,
        );
    const files = readTree(dir);
    checkLabels(result, options);
    return new WorkflowRun(result, dir, before, files);
  } finally {
    if (!options.keep) rmSync(dir, { recursive: true, force: true });
  }
}

/** Whether this run goes to `codemod/host` rather than the CLI. */
async function inProcess(workflow, options) {
  const host = options.host ?? "node";
  if (host === "quickjs") return false;
  if (host !== "node") throw new TypeError(`host must be "node" or "quickjs", not ${host}`);
  if (workflow && typeof workflow === "object" && !isUrl(workflow)) return true;
  const { addon } = await import("../runtime/addon.mjs");
  let path;
  try {
    path = addon().resolveWorkflow(toPath(workflow), options.workflowName ?? null);
  } catch (error) {
    // Without the addon, fall back to the CLI unless Node was asked for.
    if (options.host === undefined && /native addon/.test(error.message)) return false;
    throw new Error(`cannot run ${toPath(workflow)}: ${error.message}`);
  }
  // YAML workflows run in the engine.
  return /\.m?[jt]s$/.test(path);
}

async function runInProcess(workflow, options, dir) {
  const { runWorkflow: run } = await import("../runtime/host.mjs");
  let report;
  try {
    report = await run(isPath(workflow) ? toPath(workflow) : workflow, {
      target: dir,
      params: options.params ?? {},
      until: options.until,
      given: options.until !== undefined || options.given ? (options.given ?? {}) : undefined,
      approve: options.approve ?? true,
      capabilities: options.capabilities ?? [],
      workflowName: options.workflowName,
      path: options.path === undefined ? undefined : toPath(options.path),
      signal: options.signal,
      captureConsole: true,
    });
  } catch (error) {
    throw new Error(`cannot run ${describeWorkflow(workflow, options)}: ${error.message}`, {
      cause: error,
    });
  }
  return { ...report, logs: logsOf(report.events) };
}

function describeWorkflow(workflow, options) {
  if (isPath(workflow)) return toPath(workflow);
  return options.path === undefined ? "the workflow" : toPath(options.path);
}

/** Log lines from the engine's events, with the label of the step that wrote each. */
function logsOf(events) {
  const labels = new Map();
  const lines = [];
  for (const event of events) {
    if (event.event === "step_started") labels.set(event.position, event.label);
    else if (event.event === "step_log") {
      lines.push({ step: labels.get(event.position) ?? null, line: event.line });
    } else if (event.event === "run_log") lines.push({ step: null, line: event.line });
  }
  return lines;
}

class WorkflowRun {
  constructor(result, dir, before, files) {
    this.status = result.status;
    this.output = result.output;
    this.error = result.error;
    this.awaiting = result.awaiting;
    this.stoppedAt = result.stoppedAt;
    this.dir = dir;
    this.files = Object.freeze(files);
    this.changes = Object.freeze(changes(before, files));
    this.steps = Object.freeze(result.steps.map((step) => Object.freeze(step)));
    this.logs = Object.freeze(result.logs.map((log) => Object.freeze(log)));
    this.metrics = result.metrics;
    Object.defineProperty(this, "before", { value: before, enumerable: false });
  }

  /** Whether the run finished, or stopped at its `until` step. */
  get ok() {
    return this.status === "succeeded" || this.status === "stopped";
  }

  /** The first step with this label. Throws when there is none. */
  step(label) {
    const found = this.steps.find((step) => step.label === label);
    if (!found) {
      throw new Error(
        `no step is labeled ${JSON.stringify(label)}; the run's steps are: ${labels(this.steps)}`,
      );
    }
    return found;
  }

  /** A unified diff of every file the run changed. */
  diff() {
    return Object.keys(this.changes)
      .sort()
      .map((path) => unifiedDiff(path, this.before[path] ?? "", this.changes[path] ?? ""))
      .join("");
  }
}

function checkLabels(result, options) {
  const ran = result.steps;
  if (options.until !== undefined && result.status === "succeeded") {
    throw new Error(
      `until: no step is labeled ${JSON.stringify(options.until)}; the run's steps are: ${labels(ran)}`,
    );
  }
  if (result.status !== "succeeded") return;
  const unused = Object.keys(options.given ?? {}).filter(
    (label) => !ran.some((step) => step.given && step.label === label),
  );
  if (unused.length) {
    throw new Error(
      `given: no step is labeled ${unused.map((l) => JSON.stringify(l)).join(", ")}; the run's steps are: ${labels(ran)}`,
    );
  }
}

function labels(steps) {
  return steps.length ? steps.map((step) => JSON.stringify(step.label)).join(", ") : "(none)";
}

function harness(spec, signal) {
  const binary = codemodBinary();
  return new Promise((resolvePromise, reject) => {
    const child = spawn(binary, ["workflow", "harness"], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, NO_COLOR: "1", DISABLE_ANALYTICS: "true" },
      signal,
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk) => (stderr += chunk));
    child.on("error", reject);
    child.on("close", (code) => {
      const line = stdout.trim().split("\n").pop();
      if (code === 0 && line) {
        try {
          resolvePromise(JSON.parse(line));
          return;
        } catch {
          // fall through to the error below
        }
      }
      const detail = (stderr.trim() || stdout.trim() || `exit code ${code}`).replace(
        /^Error:\s*/,
        "",
      );
      reject(new Error(`cannot run ${spec.workflow}: ${detail}`));
    });
    child.stdin.end(JSON.stringify(spec));
  });
}

let binaryPath;

/** The codemod CLI binary: `CODEMOD_BINARY`, or the one this package installed. */
function codemodBinary() {
  if (process.env.CODEMOD_BINARY) return process.env.CODEMOD_BINARY;
  if (binaryPath) return binaryPath;
  const { findBinaryPath } = require("../codemod");
  const found = findBinaryPath();
  if (!found.binaryPath) {
    throw new Error(
      `cannot find the codemod binary${found.pkgName ? ` (expected the ${found.pkgName} package)` : ""}; reinstall codemod with optional dependencies, or set CODEMOD_BINARY`,
    );
  }
  binaryPath = found.binaryPath;
  return binaryPath;
}

// ------------------------------------------------------------ fixtures ---

/**
 * The fixtures in `dir`: each subdirectory with `input/` and `expected/`, or
 * `input.<ext>` and `expected.<ext>`, and optionally `params.json`. Groups of
 * fixtures may be nested.
 */
export function fixtures(dir) {
  const root = toPath(dir);
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    throw new Error(`no fixtures directory at ${root}`);
  }
  const found = [];
  // The fixtures directory may itself hold `input/` and `expected/`.
  if (isDir(join(root, "input"))) {
    found.push(
      fixture(basename(root), root, join(root, "input"), join(root, "expected"), readParams(root)),
    );
  }
  collect(root, root, found);
  return found;
}

function isDir(path) {
  return existsSync(path) && statSync(path).isDirectory();
}

function collect(root, dir, found) {
  // A fixture's own input/ and expected/ are not fixtures.
  const own = isDir(join(dir, "input")) ? new Set(["input", "expected"]) : new Set();
  const subdirs = readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && !own.has(entry.name))
    .map((entry) => join(dir, entry.name))
    .sort();
  for (const sub of subdirs) {
    const name = relative(root, sub).split(sep).join("/");
    const params = readParams(sub);
    if (isDir(join(sub, "input"))) {
      found.push(fixture(name, sub, join(sub, "input"), join(sub, "expected"), params));
      continue;
    }
    const inputs = readdirSync(sub).filter(
      (file) => basename(file, extname(file)) === "input" && statSync(join(sub, file)).isFile(),
    );
    if (inputs.length > 1) {
      throw new Error(
        `${sub} has more than one input file; use input/ and expected/ directories instead`,
      );
    }
    if (inputs.length === 1) {
      const input = join(sub, inputs[0]);
      found.push(fixture(name, sub, input, join(sub, `expected${extname(input)}`), params));
      continue;
    }
    collect(root, sub, found);
  }
}

function fixture(name, dir, input, expected, params) {
  return Object.freeze({ name, dir, input, expected, params });
}

function readParams(dir) {
  const path = join(dir, "params.json");
  if (!existsSync(path)) return {};
  const params = JSON.parse(readFileSync(path, "utf8"));
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    throw new Error(`${path} must hold a JSON object`);
  }
  return params;
}

// ----------------------------------------------------------- assertions ---

/**
 * Assert that a run left the expected files. `actual` is a run, a directory,
 * or files by path; `expected` is a directory, a single file, or files by
 * path. With `CODEMOD_UPDATE_FIXTURES=1`, an expected directory or file is
 * rewritten to match instead.
 */
export function assertTree(actual, expected, options = {}) {
  if (actual instanceof WorkflowRun && !actual.ok) {
    throw new AssertionError({
      message: describeFailure(actual),
      actual: actual.status,
      expected: "succeeded",
      operator: "assertTree",
    });
  }
  const actualTree = actual instanceof WorkflowRun ? actual.files : treeOf(actual);
  const expectedPath = isPath(expected) ? toPath(expected) : null;
  const single =
    expectedPath !== null && existsSync(expectedPath) && statSync(expectedPath).isFile();

  if (expectedPath !== null && updating()) {
    // A file (`expected.ts`) when one exists or the name has an extension;
    // otherwise a directory.
    const asFile = single || (!existsSync(expectedPath) && extname(expectedPath) !== "");
    if (asFile) {
      writeFileSync(expectedPath, soleFile(actualTree, expectedPath)[1]);
    } else {
      rmSync(expectedPath, { recursive: true, force: true });
      writeTree(expectedPath, actualTree);
    }
    return;
  }

  if (single) {
    const [path, text] = soleFile(actualTree, expectedPath);
    const want = normalize(readFileSync(expectedPath, "utf8"));
    if (!same(text, want, options)) {
      fail(
        `${path} differs from ${expectedPath}`,
        text,
        want,
        unifiedDiff(path, want, text),
        options,
      );
    }
    return;
  }

  const expectedTree = treeOf(expected);
  const paths = [...new Set([...Object.keys(expectedTree), ...Object.keys(actualTree)])].sort();
  const differing = paths.filter(
    (path) =>
      !(path in expectedTree) ||
      !(path in actualTree) ||
      !same(actualTree[path], expectedTree[path], options),
  );
  if (!differing.length) return;

  const report = differing.map((path) => {
    if (!(path in actualTree)) return `missing ${path}\n`;
    if (!(path in expectedTree)) return `unexpected ${path}\n`;
    return unifiedDiff(path, expectedTree[path], actualTree[path]);
  });
  const only = differing.length === 1 && differing[0] in actualTree && differing[0] in expectedTree;
  const title = only
    ? `${differing[0]} differs from the expected output`
    : `${differing.length} ${differing.length === 1 ? "file differs" : "files differ"} from the expected output`;
  if (only) {
    fail(title, actualTree[differing[0]], expectedTree[differing[0]], report.join(""), options);
  }
  fail(
    title,
    render(actualTree, differing),
    render(expectedTree, differing),
    report.join(""),
    options,
  );
}

function fail(title, actual, expected, diff, options) {
  const message = options.message ? `${options.message}\n${title}` : title;
  throw new AssertionError({
    message: `${message}\n\n${diff.trimEnd()}\n${updateHint()}`,
    actual,
    expected,
    operator: "assertTree",
  });
}

function describeFailure(run) {
  const what = {
    failed: `the run failed: ${run.error}`,
    canceled: "the run was canceled",
    "awaiting-approval": `the run is waiting for an approval: ${run.awaiting}`,
  }[run.status];
  const logs = run.logs.length
    ? `\n\nlogs:\n${run.logs.map((log) => `  ${log.step ? `[${log.step}] ` : ""}${log.line}`).join("\n")}`
    : "";
  return `${what ?? `the run ended as ${run.status}`}${logs}`;
}

function updateHint() {
  return "(set CODEMOD_UPDATE_FIXTURES=1 to update expected files from the run)";
}

function updating() {
  const value = process.env.CODEMOD_UPDATE_FIXTURES;
  return value === "1" || value === "true";
}

function same(a, b, options) {
  if (options.ignoreWhitespace) return squash(a) === squash(b);
  return a === b;
}

function squash(text) {
  return text
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .trimEnd();
}

function soleFile(tree, expectedPath) {
  const entries = Object.entries(tree);
  if (entries.length === 1) return entries[0];
  throw new AssertionError({
    message: `${expectedPath} is one file, but the run left ${entries.length} (${Object.keys(tree).join(", ") || "none"}); compare against a directory to test more than one file`,
    actual: Object.keys(tree),
    expected: [basename(expectedPath)],
    operator: "assertTree",
  });
}

function render(tree, paths) {
  return paths
    .map((path) => (path in tree ? `// ${path}\n${tree[path]}` : `// ${path} (no file)\n`))
    .join("\n");
}

// --------------------------------------------------------------- trees ---

function treeOf(value) {
  if (isPath(value)) return readTree(toPath(value));
  if (value && typeof value === "object") {
    const tree = {};
    for (const [path, text] of Object.entries(value)) {
      if (text === null) continue;
      if (typeof text !== "string") throw new TypeError(`file ${path} must be a string`);
      tree[path.replace(/^\.\//, "")] = normalize(text);
    }
    return tree;
  }
  throw new TypeError("expected a directory path, a URL, or files by path");
}

function materialize(target, dir) {
  if (isPath(target)) {
    const path = toPath(target);
    if (!existsSync(path)) throw new Error(`target ${path} does not exist`);
    if (statSync(path).isFile()) cpSync(path, join(dir, basename(path)));
    else cpSync(path, dir, { recursive: true });
    return;
  }
  writeTree(dir, treeOf(target));
}

function writeTree(dir, tree) {
  mkdirSync(dir, { recursive: true });
  for (const [path, text] of Object.entries(tree)) {
    const file = join(dir, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
  }
}

function readTree(dir) {
  const tree = {};
  if (!existsSync(dir)) return tree;
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) walk(path);
      else tree[relative(dir, path).split(sep).join("/")] = normalize(readFileSync(path, "utf8"));
    }
  };
  walk(dir);
  return Object.fromEntries(Object.entries(tree).sort(([a], [b]) => (a < b ? -1 : 1)));
}

function changes(before, after) {
  const changed = {};
  for (const [path, text] of Object.entries(after)) {
    if (before[path] !== text) changed[path] = text;
  }
  for (const path of Object.keys(before)) {
    if (!(path in after)) changed[path] = null;
  }
  return changed;
}

function normalize(text) {
  return text.replace(/\r\n/g, "\n");
}

function toPath(value) {
  if (isUrl(value)) return fileURLToPath(value.href);
  if (typeof value === "string" && value.startsWith("file:")) return fileURLToPath(value);
  if (typeof value === "string") return resolve(value);
  throw new TypeError("expected a path or a file URL");
}

// Not `instanceof URL`: test environments may have their own URL class.
function isUrl(value) {
  return value !== null && typeof value === "object" && typeof value.href === "string";
}

function isPath(value) {
  return typeof value === "string" || isUrl(value);
}

// ---------------------------------------------------------------- diff ---

/** A unified diff of two texts, by line, with three lines of context. */
function unifiedDiff(path, from, to) {
  const a = splitLines(from);
  const b = splitLines(to);
  const ops = diffLines(a, b);
  if (ops.every((op) => op[0] === " ")) return "";
  const hunks = [];
  const context = 3;
  let i = 0;
  while (i < ops.length) {
    if (ops[i][0] === " ") {
      i++;
      continue;
    }
    let start = Math.max(0, i - context);
    let end = i;
    // Extend the hunk while changes are within 2 * context lines of each other.
    while (end < ops.length) {
      let next = end;
      while (next < ops.length && ops[next][0] === " ") next++;
      if (next >= ops.length || next - end > context * 2) break;
      while (next < ops.length && ops[next][0] !== " ") next++;
      end = next;
    }
    end = Math.min(ops.length, end + context);
    hunks.push([start, end]);
    i = end;
  }
  let out = `--- expected/${path}\n+++ actual/${path}\n`;
  for (const [start, end] of hunks) {
    let aLine = 1;
    let bLine = 1;
    for (let k = 0; k < start; k++) {
      if (ops[k][0] !== "+") aLine++;
      if (ops[k][0] !== "-") bLine++;
    }
    const slice = ops.slice(start, end);
    const aCount = slice.filter((op) => op[0] !== "+").length;
    const bCount = slice.filter((op) => op[0] !== "-").length;
    out += `@@ -${aLine},${aCount} +${bLine},${bCount} @@\n`;
    for (const [sign, line] of slice) out += `${sign}${line}\n`;
  }
  return out;
}

function splitLines(text) {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** Longest-common-subsequence line diff: a list of [" " | "-" | "+", line]. */
function diffLines(a, b) {
  const n = a.length;
  const m = b.length;
  const table = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      table[i][j] =
        a[i] === b[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
    }
  }
  const ops = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      ops.push([" ", a[i++]]);
      j++;
    } else if (table[i + 1][j] >= table[i][j + 1]) {
      ops.push(["-", a[i++]]);
    } else {
      ops.push(["+", b[j++]]);
    }
  }
  while (i < n) ops.push(["-", a[i++]]);
  while (j < m) ops.push(["+", b[j++]]);
  return ops;
}

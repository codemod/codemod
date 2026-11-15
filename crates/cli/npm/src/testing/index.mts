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

/**
 * Run workflows from any test runner and compare the files they leave.
 *
 * ```ts
 * import { test } from "node:test";
 * import { runWorkflow, fixtures, assertTree } from "codemod/testing";
 *
 * for (const fixture of fixtures("tests")) {
 *   test(fixture.name, async () => {
 *     const run = await runWorkflow("workflow.ts", { target: fixture.input, params: fixture.params });
 *     assertTree(run, fixture.expected);
 *   });
 * }
 * ```
 *
 * Each run copies its target into a temporary directory first, so fixtures
 * are never changed. Paths are resolved against the current directory;
 * pass `new URL("./workflow.ts", import.meta.url)` to resolve against the
 * test file instead.
 *
 * TypeScript workflows run in the test's own process, so breakpoints,
 * coverage and mocks work in the workflow's code. `host: "quickjs"` runs them
 * with the codemod CLI instead, in the sandboxed runtime `codemod run` uses.
 *
 * @module
 */

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

import type { EngineEvent } from "../runtime/addon.mts";
import type { WorkflowModule as HostWorkflowModule } from "../runtime/host.mts";

const require = createRequire(import.meta.url);

// --------------------------------------------------------------- types ---

export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

/** File contents by path, relative to the target (`"src/a.ts"`). */
export type FileTree = { readonly [path: string]: string };

/** A `URL` (`new URL("./tests", import.meta.url)`); only `file:` URLs are paths. */
export interface UrlLike {
  readonly href: string;
}

/** A path, or a `file:` URL. */
export type PathLike = string | UrlLike;

/** An `AbortSignal`. */
export interface AbortSignalLike {
  readonly aborted: boolean;
}

/** A workflow module: what `import("./workflow.ts")` gives. */
export interface WorkflowModule {
  readonly default: (...args: never[]) => unknown;
}

/** A capability a transform may use beyond those codemod.yaml declares. */
export type Capability = "fs" | "fetch" | "child_process";

export interface RunWorkflowOptions {
  /**
   * The files the workflow runs on: a directory or a single file (copied
   * first), or files by path.
   */
  target: PathLike | FileTree;
  /** The workflow's parameters. */
  params?: Record<string, JsonValue>;
  /**
   * Stop after the step with this label: steps the workflow starts after it
   * do not run. The run's status is then `"stopped"`.
   */
  until?: string;
  /**
   * Results for steps that should not run, by label. The workflow gets the
   * given value as the step's result.
   */
  given?: Record<string, JsonValue>;
  /** Give every approval the workflow asks for. Default `true`. */
  approve?: boolean;
  /** Capabilities granted on top of those codemod.yaml declares. */
  capabilities?: Capability[];
  /** Which workflow to run when codemod.yaml defines several. */
  workflowName?: string;
  /** Keep the temporary directory the run changed (`run.dir`). */
  keep?: boolean;
  /** Stops the run. */
  signal?: AbortSignalLike;
  /**
   * Where the workflow runs. `"node"` (the default for TypeScript workflows):
   * in this process. `"quickjs"`: with the codemod CLI, in the sandboxed
   * runtime `codemod run` uses, where capabilities are enforced. YAML
   * workflows always run with the CLI.
   */
  host?: "node" | "quickjs";
  /** The workflow file, when `workflow` is the module itself. */
  path?: PathLike;
}

export type RunStatus = "succeeded" | "failed" | "canceled" | "awaiting-approval" | "stopped";

export interface StepRun {
  readonly position: number;
  readonly label: string;
  /**
   * `"partial"`: the step finished, but some of its files failed; `error`
   * says how many.
   */
  readonly status: "succeeded" | "partial" | "failed";
  readonly result: JsonValue | null;
  readonly error: string | null;
  /** The step did not run: its result came from `given`. */
  readonly given: boolean;
}

export interface LogLine {
  /** Label of the step that logged it; `null` for the workflow body. */
  readonly step: string | null;
  readonly line: string;
}

export interface WorkflowRun {
  readonly status: RunStatus;
  /** Whether the run succeeded, or stopped at its `until` step. */
  readonly ok: boolean;
  /** What the workflow returned, when it succeeded. */
  readonly output: JsonValue | null;
  /** Why the run failed. */
  readonly error: string | null;
  /** The approval the run waits for (`approve: false`). */
  readonly awaiting: string | null;
  /** The `until` step the run stopped after. */
  readonly stoppedAt: string | null;
  /** The directory the run changed; removed afterwards unless `keep` is set. */
  readonly dir: string;
  /** Every file after the run. */
  readonly files: FileTree;
  /** Files the run wrote or created, with their new text; `null` for files it removed. */
  readonly changes: { readonly [path: string]: string | null };
  /** Every step, in the order the workflow started them. */
  readonly steps: readonly StepRun[];
  /** Console output, from steps, callbacks and the workflow body. */
  readonly logs: readonly LogLine[];
  /** Metrics the transforms recorded. */
  readonly metrics: Record<string, unknown>;
  /** The first step with this label. Throws when there is none. */
  step(label: string): StepRun;
  /** A unified diff of every file the run changed. */
  diff(): string;
}

export interface Fixture {
  /** The fixture's directory, relative to the fixtures directory (`"renames/default-export"`). */
  readonly name: string;
  readonly dir: string;
  /** `input/`, or the `input.<ext>` file: pass it as `target`. */
  readonly input: string;
  /** `expected/`, or the `expected.<ext>` file: pass it to `assertTree`. */
  readonly expected: string;
  /** From `params.json`, or empty. */
  readonly params: Record<string, JsonValue>;
}

export interface AssertTreeOptions {
  /** Ignore whitespace at the ends of lines and of files. */
  ignoreWhitespace?: boolean;
  /** Put in front of the failure message. */
  message?: string;
}

/** File contents by path, while they are being put together. */
type Tree = Record<string, string>;

/** What a run reports, from `codemod/host` or the CLI's `workflow harness`. */
interface RunResult {
  status: RunStatus;
  output: JsonValue | null;
  error: string | null;
  awaiting: string | null;
  stoppedAt: string | null;
  steps: StepRun[];
  logs: LogLine[];
  metrics: Record<string, unknown>;
}

/** The test `codemod workflow harness` reads on stdin (`CaseRequest` in the CLI). */
interface HarnessSpec {
  workflow: string;
  workflowName: string | undefined;
  target: string;
  params: Record<string, JsonValue>;
  until: string | undefined;
  given: Record<string, JsonValue>;
  approve: boolean;
  capabilities: Capability[];
}

/** What the `codemod` launcher next to this package exports. */
interface Launcher {
  findBinaryPath(): { binaryPath?: string | null; pkgName?: string | null };
}

// ------------------------------------------------------------ running ---

/**
 * Run a workflow (a workflow file, a package directory, or the workflow
 * module itself, with `options.path`) on a copy of `options.target`. Resolves for every outcome; check `run.status`, or use
 * `assertTree`, which fails when the run did not succeed. Rejects when the
 * workflow cannot run at all (for example it does not build), or when an
 * `until` or `given` label matches no step.
 */
export async function runWorkflow(
  workflow: PathLike | WorkflowModule,
  options: RunWorkflowOptions,
): Promise<WorkflowRun> {
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
    return new Run(result, dir, before, files);
  } finally {
    if (!options.keep) rmSync(dir, { recursive: true, force: true });
  }
}

/** Whether this run goes to `codemod/host` rather than the CLI. */
async function inProcess(
  workflow: PathLike | WorkflowModule,
  options: RunWorkflowOptions,
): Promise<boolean> {
  const host: string = options.host ?? "node";
  if (host === "quickjs") return false;
  if (host !== "node") throw new TypeError(`host must be "node" or "quickjs", not ${host}`);
  if (!isPath(workflow)) return true;
  const { addon } = await import("../runtime/addon.mts");
  let path;
  try {
    path = addon().resolveWorkflow(toPath(workflow), options.workflowName ?? null);
  } catch (error) {
    // Without the addon, fall back to the CLI unless Node was asked for.
    if (options.host === undefined && /native addon/.test(messageOf(error))) return false;
    throw new Error(`cannot run ${toPath(workflow)}: ${messageOf(error)}`);
  }
  // YAML workflows run in the engine.
  return /\.m?[jt]s$/.test(path);
}

async function runInProcess(
  workflow: PathLike | WorkflowModule,
  options: RunWorkflowOptions,
  dir: string,
): Promise<RunResult> {
  const { runWorkflow: run } = await import("../runtime/host.mts");
  let report;
  try {
    // The host calls the module's default export with the run's input.
    report = await run(isPath(workflow) ? toPath(workflow) : (workflow as HostWorkflowModule), {
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
    throw new Error(`cannot run ${describeWorkflow(workflow, options)}: ${messageOf(error)}`, {
      cause: error,
    });
  }
  return { ...report, steps: [...report.steps], logs: logsOf(report.events) };
}

function describeWorkflow(
  workflow: PathLike | WorkflowModule,
  options: RunWorkflowOptions,
): string {
  if (isPath(workflow)) return toPath(workflow);
  return options.path === undefined ? "the workflow" : toPath(options.path);
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Log lines from the engine's events, with the label of the step that wrote each. */
function logsOf(events: readonly EngineEvent[]): LogLine[] {
  const labels = new Map<unknown, string>();
  const lines: LogLine[] = [];
  for (const event of events) {
    if (event.event === "step_started") labels.set(event.position, String(event.label));
    else if (event.event === "step_log") {
      lines.push({ step: labels.get(event.position) ?? null, line: String(event.line) });
    } else if (event.event === "run_log") lines.push({ step: null, line: String(event.line) });
  }
  return lines;
}

class Run implements WorkflowRun {
  readonly status: RunStatus;
  readonly output: JsonValue | null;
  readonly error: string | null;
  readonly awaiting: string | null;
  readonly stoppedAt: string | null;
  readonly dir: string;
  readonly files: FileTree;
  readonly changes: { readonly [path: string]: string | null };
  readonly steps: readonly StepRun[];
  readonly logs: readonly LogLine[];
  readonly metrics: Record<string, unknown>;
  /** Every file before the run; not listed with the run's own properties. */
  declare private readonly before: FileTree;

  constructor(result: RunResult, dir: string, before: Tree, files: Tree) {
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
  get ok(): boolean {
    return this.status === "succeeded" || this.status === "stopped";
  }

  /** The first step with this label. Throws when there is none. */
  step(label: string): StepRun {
    const found = this.steps.find((step) => step.label === label);
    if (!found) {
      throw new Error(
        `no step is labeled ${JSON.stringify(label)}; the run's steps are: ${labels(this.steps)}`,
      );
    }
    return found;
  }

  /** A unified diff of every file the run changed. */
  diff(): string {
    return Object.keys(this.changes)
      .sort()
      .map((path) => unifiedDiff(path, this.before[path] ?? "", this.changes[path] ?? ""))
      .join("");
  }
}

function checkLabels(result: RunResult, options: RunWorkflowOptions): void {
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

function labels(steps: readonly StepRun[]): string {
  return steps.length ? steps.map((step) => JSON.stringify(step.label)).join(", ") : "(none)";
}

function harness(spec: HarnessSpec, signal: AbortSignalLike | undefined): Promise<RunResult> {
  const binary = codemodBinary();
  return new Promise((resolvePromise, reject) => {
    const child = spawn(binary, ["workflow", "harness"], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, NO_COLOR: "1", DISABLE_ANALYTICS: "true" },
      // Stopping a process takes a real signal.
      signal: signal as AbortSignal | undefined,
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => (stderr += chunk));
    child.on("error", (error) => {
      // Aborting stops the CLI; the run then ended as canceled, as it does
      // when Node.js runs the workflow.
      if (signal?.aborted) {
        resolvePromise({
          status: "canceled",
          output: null,
          error: null,
          awaiting: null,
          stoppedAt: null,
          steps: [],
          logs: [],
          metrics: {},
        });
      } else {
        reject(error);
      }
    });
    child.on("close", (code) => {
      const line = stdout.trim().split("\n").pop();
      if (code === 0 && line) {
        try {
          resolvePromise(JSON.parse(line) as RunResult);
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

let binaryPath: string | undefined;

/** The codemod CLI binary: `CODEMOD_BINARY`, or the one this package installed. */
function codemodBinary(): string {
  if (process.env.CODEMOD_BINARY) return process.env.CODEMOD_BINARY;
  if (binaryPath) return binaryPath;
  const { findBinaryPath } = require("../codemod") as Launcher;
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
 * The fixtures in a directory: each subdirectory with `input/` and
 * `expected/` directories, or `input.<ext>` and `expected.<ext>` files (the
 * layouts `codemod jssg test` reads), and optionally `params.json`. Groups of
 * fixtures may be nested.
 */
export function fixtures(dir: PathLike): Fixture[] {
  const root = toPath(dir);
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    throw new Error(`no fixtures directory at ${root}`);
  }
  const found: Fixture[] = [];
  // The fixtures directory may itself hold `input/` and `expected/`.
  if (isDir(join(root, "input"))) {
    found.push(
      fixture(basename(root), root, join(root, "input"), join(root, "expected"), readParams(root)),
    );
  }
  collect(root, root, found);
  return found;
}

function isDir(path: string): boolean {
  return existsSync(path) && statSync(path).isDirectory();
}

function collect(root: string, dir: string, found: Fixture[]): void {
  // A fixture's own input/ and expected/ are not fixtures.
  const own = new Set(isDir(join(dir, "input")) ? ["input", "expected"] : []);
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
    const [inputFile, ...more] = readdirSync(sub).filter(
      (file) => basename(file, extname(file)) === "input" && statSync(join(sub, file)).isFile(),
    );
    if (more.length) {
      throw new Error(
        `${sub} has more than one input file; use input/ and expected/ directories instead`,
      );
    }
    if (inputFile !== undefined) {
      const input = join(sub, inputFile);
      found.push(fixture(name, sub, input, join(sub, `expected${extname(input)}`), params));
      continue;
    }
    collect(root, sub, found);
  }
}

function fixture(
  name: string,
  dir: string,
  input: string,
  expected: string,
  params: Record<string, JsonValue>,
): Fixture {
  return Object.freeze({ name, dir, input, expected, params });
}

function readParams(dir: string): Record<string, JsonValue> {
  const path = join(dir, "params.json");
  if (!existsSync(path)) return {};
  const params: unknown = JSON.parse(readFileSync(path, "utf8"));
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    throw new Error(`${path} must hold a JSON object`);
  }
  return params as Record<string, JsonValue>;
}

// ----------------------------------------------------------- assertions ---

/**
 * Assert that `actual` holds exactly the expected files. Throws an
 * `AssertionError` with `actual` and `expected` set and a diff in the
 * message.
 *
 * - `actual`: a run (which must have succeeded or stopped), a directory, or
 *   files by path.
 * - `expected`: a directory, files by path, or a single file, in which case
 *   the run must have left exactly one file.
 *
 * Line endings are compared as `\n`. With `CODEMOD_UPDATE_FIXTURES=1`, an
 * expected directory or file is rewritten to match `actual` instead.
 */
export function assertTree(
  actual: WorkflowRun | PathLike | FileTree,
  expected: PathLike | FileTree,
  options: AssertTreeOptions = {},
): void {
  if (actual instanceof Run && !actual.ok) {
    throw new AssertionError({
      message: describeFailure(actual),
      actual: actual.status,
      expected: "succeeded",
      operator: "assertTree",
    });
  }
  const actualTree = actual instanceof Run ? actual.files : treeOf(actual);
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

  if (expectedPath !== null && single) {
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
  // What one tree has at a path that the other differs at; a file is never
  // missing from both.
  const differing = paths.flatMap((path) => {
    const [got, want] = [actualTree[path], expectedTree[path]];
    return got !== undefined && want !== undefined && same(got, want, options)
      ? []
      : [{ path, got, want }];
  });
  const [first, ...rest] = differing;
  if (!first) return;

  const report = differing
    .map(({ path, got, want }) => {
      if (got === undefined) return `missing ${path}\n`;
      if (want === undefined) return `unexpected ${path}\n`;
      return unifiedDiff(path, want, got);
    })
    .join("");
  if (!rest.length && first.got !== undefined && first.want !== undefined) {
    fail(`${first.path} differs from the expected output`, first.got, first.want, report, options);
  }
  const changed = differing.map(({ path }) => path);
  fail(
    `${differing.length} ${differing.length === 1 ? "file differs" : "files differ"} from the expected output`,
    render(actualTree, changed),
    render(expectedTree, changed),
    report,
    options,
  );
}

function fail(
  title: string,
  actual: string,
  expected: string,
  diff: string,
  options: AssertTreeOptions,
): never {
  const message = options.message ? `${options.message}\n${title}` : title;
  throw new AssertionError({
    message: `${message}\n\n${diff.trimEnd()}\n${updateHint()}`,
    actual,
    expected,
    operator: "assertTree",
  });
}

function describeFailure(run: WorkflowRun): string {
  const what =
    run.status === "failed"
      ? `the run failed: ${run.error}`
      : run.status === "canceled"
        ? "the run was canceled"
        : run.status === "awaiting-approval"
          ? `the run is waiting for an approval: ${run.awaiting}`
          : `the run ended as ${run.status}`;
  const logs = run.logs.length
    ? `\n\nlogs:\n${run.logs.map((log) => `  ${log.step ? `[${log.step}] ` : ""}${log.line}`).join("\n")}`
    : "";
  return `${what}${logs}`;
}

function updateHint(): string {
  return "(set CODEMOD_UPDATE_FIXTURES=1 to update expected files from the run)";
}

function updating(): boolean {
  const value = process.env.CODEMOD_UPDATE_FIXTURES;
  return value === "1" || value === "true";
}

function same(a: string, b: string, options: AssertTreeOptions): boolean {
  if (options.ignoreWhitespace) return squash(a) === squash(b);
  return a === b;
}

function squash(text: string): string {
  return text
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .trimEnd();
}

function soleFile(tree: FileTree, expectedPath: string): [path: string, text: string] {
  const [only, ...others] = Object.entries(tree);
  if (only && !others.length) return only;
  throw new AssertionError({
    message: `${expectedPath} is one file, but the run left ${Object.keys(tree).length} (${Object.keys(tree).join(", ") || "none"}); compare against a directory to test more than one file`,
    actual: Object.keys(tree),
    expected: [basename(expectedPath)],
    operator: "assertTree",
  });
}

function render(tree: FileTree, paths: string[]): string {
  return paths
    .map((path) => (path in tree ? `// ${path}\n${tree[path]}` : `// ${path} (no file)\n`))
    .join("\n");
}

// --------------------------------------------------------------- trees ---

function treeOf(value: unknown): Tree {
  if (isPath(value)) return readTree(toPath(value));
  if (value && typeof value === "object") {
    const tree: Tree = {};
    for (const [path, text] of Object.entries(value)) {
      if (text === null) continue;
      if (typeof text !== "string") throw new TypeError(`file ${path} must be a string`);
      tree[path.replace(/^\.\//, "")] = normalize(text);
    }
    return tree;
  }
  throw new TypeError("expected a directory path, a URL, or files by path");
}

function materialize(target: PathLike | FileTree, dir: string): void {
  if (isPath(target)) {
    const path = toPath(target);
    if (!existsSync(path)) throw new Error(`target ${path} does not exist`);
    if (statSync(path).isFile()) cpSync(path, join(dir, basename(path)));
    else cpSync(path, dir, { recursive: true });
    return;
  }
  writeTree(dir, treeOf(target));
}

function writeTree(dir: string, tree: FileTree): void {
  mkdirSync(dir, { recursive: true });
  for (const [path, text] of Object.entries(tree)) {
    const file = join(dir, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
  }
}

function readTree(dir: string): Tree {
  const tree: Tree = {};
  if (!existsSync(dir)) return tree;
  const walk = (current: string) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) walk(path);
      else tree[relative(dir, path).split(sep).join("/")] = normalize(readFileSync(path, "utf8"));
    }
  };
  walk(dir);
  return Object.fromEntries(Object.entries(tree).sort(([a], [b]) => (a < b ? -1 : 1)));
}

function changes(before: FileTree, after: FileTree): Record<string, string | null> {
  const changed: Record<string, string | null> = {};
  for (const [path, text] of Object.entries(after)) {
    if (before[path] !== text) changed[path] = text;
  }
  for (const path of Object.keys(before)) {
    if (!(path in after)) changed[path] = null;
  }
  return changed;
}

function normalize(text: string): string {
  return text.replace(/\r\n/g, "\n");
}

function toPath(value: unknown): string {
  if (isUrl(value)) return fileURLToPath(value.href);
  if (typeof value === "string" && value.startsWith("file:")) return fileURLToPath(value);
  if (typeof value === "string") return resolve(value);
  throw new TypeError("expected a path or a file URL");
}

// Not `instanceof URL`: test environments may have their own URL class.
function isUrl(value: unknown): value is UrlLike {
  return value !== null && typeof value === "object" && typeof (value as UrlLike).href === "string";
}

function isPath(value: unknown): value is PathLike {
  return typeof value === "string" || isUrl(value);
}

// ---------------------------------------------------------------- diff ---

/** One line of a diff: kept (" "), only in the first text ("-"), or only in the second ("+"). */
type Op = [sign: " " | "-" | "+", line: string];

/** A unified diff of two texts, by line, with three lines of context. */
function unifiedDiff(path: string, from: string, to: string): string {
  const ops = diffLines(splitLines(from), splitLines(to));
  const kept = (index: number) => ops[index]?.[0] === " ";
  if (ops.every(([sign]) => sign === " ")) return "";
  const hunks: [start: number, end: number][] = [];
  const context = 3;
  let i = 0;
  while (i < ops.length) {
    if (kept(i)) {
      i++;
      continue;
    }
    const start = Math.max(0, i - context);
    let end = i;
    // Extend the hunk while changes are within 2 * context lines of each other.
    while (end < ops.length) {
      let next = end;
      while (next < ops.length && kept(next)) next++;
      if (next >= ops.length || next - end > context * 2) break;
      while (next < ops.length && !kept(next)) next++;
      end = next;
    }
    end = Math.min(ops.length, end + context);
    hunks.push([start, end]);
    i = end;
  }
  let out = `--- expected/${path}\n+++ actual/${path}\n`;
  for (const [start, end] of hunks) {
    const before = ops.slice(0, start);
    const aLine = 1 + before.filter(([sign]) => sign !== "+").length;
    const bLine = 1 + before.filter(([sign]) => sign !== "-").length;
    const slice = ops.slice(start, end);
    const aCount = slice.filter(([sign]) => sign !== "+").length;
    const bCount = slice.filter(([sign]) => sign !== "-").length;
    out += `@@ -${aLine},${aCount} +${bLine},${bCount} @@\n`;
    for (const [sign, line] of slice) out += `${sign}${line}\n`;
  }
  return out;
}

function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** Longest-common-subsequence line diff. */
function diffLines(a: string[], b: string[]): Op[] {
  const n = a.length;
  const m = b.length;
  // lengths[i * width + j]: the longest common subsequence of a[i..] and b[j..].
  const width = m + 1;
  const lengths = new Uint32Array((n + 1) * width);
  const length = (i: number, j: number) => lengths[i * width + j] ?? 0;
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lengths[i * width + j] =
        a[i] === b[j] ? length(i + 1, j + 1) + 1 : Math.max(length(i + 1, j), length(i, j + 1));
    }
  }
  const ops: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    const [left, right] = [a[i] ?? "", b[j] ?? ""];
    if (left === right) {
      ops.push([" ", left]);
      i++;
      j++;
    } else if (length(i + 1, j) >= length(i, j + 1)) {
      ops.push(["-", left]);
      i++;
    } else {
      ops.push(["+", right]);
      j++;
    }
  }
  for (; i < n; i++) ops.push(["-", a[i] ?? ""]);
  for (; j < m; j++) ops.push(["+", b[j] ?? ""]);
  return ops;
}

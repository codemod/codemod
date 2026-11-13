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
  readonly status: "succeeded" | "failed";
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

/**
 * Run a workflow (a workflow file, a package directory, or the workflow
 * module itself, with `options.path`) on a copy of `options.target`. Resolves for every outcome; check `run.status`, or use
 * `assertTree`, which fails when the run did not succeed. Rejects when the
 * workflow cannot run at all (for example it does not build), or when an
 * `until` or `given` label matches no step.
 */
export function runWorkflow(
  workflow: PathLike | WorkflowModule,
  options: RunWorkflowOptions,
): Promise<WorkflowRun>;

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

/**
 * The fixtures in a directory: each subdirectory with `input/` and
 * `expected/` directories, or `input.<ext>` and `expected.<ext>` files (the
 * layouts `codemod jssg test` and `codemod workflow test` read). Groups of
 * fixtures may be nested.
 */
export function fixtures(dir: PathLike): Fixture[];

export interface AssertTreeOptions {
  /** Ignore whitespace at the ends of lines and of files. */
  ignoreWhitespace?: boolean;
  /** Put in front of the failure message. */
  message?: string;
}

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
  options?: AssertTreeOptions,
): void;

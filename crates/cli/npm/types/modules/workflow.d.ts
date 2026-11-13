// oxlint-disable triple-slash-reference
/// <reference path="./metrics.d.ts" />

declare module "codemod:workflow" {
  /**
   * Workflow Step Outputs Module
   *
   * This module provides functions to store and retrieve step outputs
   * across workflow executions, similar to GitHub Actions.
   */

  /**
   * Sets a step output value.
   *
   * Step outputs allow you to share data between steps in a workflow.
   * Similar to GitHub Actions, you can reference outputs like:
   * ${{ steps.my_step.outputs.my_output }}
   *
   * In native mode, outputs are appended to the file specified by
   * the STEP_OUTPUTS environment variable.
   *
   * In WASM mode, outputs are stored in memory.
   *
   * @param stepId - The unique identifier of the step
   * @param outputName - The name of the output variable
   * @param value - The value to store (as a string)
   *
   * @deprecated Use `setState` instead for structured state that doesn't require manual JSON serialization.
   *
   * @example
   * ```typescript
   * import { setStepOutput } from 'codemod:workflow';
   *
   * // In a step with id="build"
   * setStepOutput('build', 'version', '1.2.3');
   * setStepOutput('build', 'artifact_path', '/tmp/build/app.zip');
   *
   * // Later steps can access these via:
   * // ${{ steps.build.outputs.version }}
   * // ${{ steps.build.outputs.artifact_path }}
   * ```
   */
  export function setStepOutput(outputName: string, value: string): void;

  /**
   * Gets a step output value.
   *
   * Returns the value as a string if found, or null if the output doesn't exist.
   *
   * @param stepId - The unique identifier of the step
   * @param outputName - The name of the output variable
   * @returns The value as a string, or null if not found
   *
   * @deprecated Use `getState` instead for structured state that doesn't require manual JSON parsing.
   *
   * @example
   * ```typescript
   * import { getStepOutput } from 'codemod:workflow';
   *
   * // Get output from a previous step
   * const version = getStepOutput('build', 'version');
   * console.log(version); // '1.2.3'
   *
   * // Check if output exists
   * const artifactPath = getStepOutput('build', 'artifact_path');
   * if (artifactPath) {
   *   console.log(`Artifact ready at: ${artifactPath}`);
   * }
   * ```
   */
  export function getStepOutput(stepId: string, outputName: string): string | null;

  /**
   * Gets a step output value, or sets it if it doesn't exist (atomically).
   *
   * This function is useful in concurrent scenarios where multiple files are processed
   * in parallel. The first thread to call this function will set the value if it doesn't
   * exist, and subsequent calls will return the existing value.
   *
   * This operation is atomic, preventing race conditions when multiple threads try to
   * access/set the same output simultaneously.
   *
   * @param stepId - The unique identifier of the step
   * @param outputName - The name of the output variable
   * @param defaultValue - The value to set if the output doesn't exist
   * @returns The existing value (if found) or the newly set default value
   *
   * @deprecated Use `setState`/`getState` with `acquireLock` instead for structured state.
   *
   * @example
   * ```typescript
   * import { getOrSetStepOutput, setStepOutput } from 'codemod:workflow';
   *
   * // Multiple files being processed in parallel
   * async function transform(root: SgRoot<JS>): Promise<string | null> {
   *   const source = root.root().text();
   *   const newFiles = source.split("\n").filter(line => line.trim() !== "");
   *
   *   // Get existing files or initialize with empty array
   *   const existingFilesStr = getOrSetStepOutput("scan-files", "allFiles", "[]");
   *   const existingFiles = JSON.parse(existingFilesStr);
   *
   *   // Merge with existing files
   *   const allFiles = [...existingFiles, ...newFiles];
   *
   *   // Update the output
   *   setStepOutput("allFiles", JSON.stringify(allFiles));
   *
   *   return null;
   * }
   * ```
   */
  export function getOrSetStepOutput(
    stepId: string,
    outputName: string,
    defaultValue: string,
  ): string;

  /**
   * Sets a named state value that is shared across all parallel file executions.
   *
   * Values are stored as native JavaScript objects internally (no JSON serialization needed).
   * When `persist` is true (the default), the value will be persisted at the end of the
   * execution batch and available to subsequent workflow steps.
   *
   * @param name - The state key
   * @param value - Any JSON-serializable value
   * @param persist - Whether to persist across workflow steps (default: true)
   *
   * @example
   * ```typescript
   * import { setState } from 'codemod:workflow';
   *
   * setState("migratedFiles", ["src/App.tsx", "src/utils.ts"]);
   * setState("stats", { totalFiles: 42, modified: 5 });
   * setState("tempCounter", 0, false); // transient, not persisted
   * ```
   */
  export function setState<T>(name: string, value: T, persist?: boolean): void;

  /**
   * Gets a named state value, or `undefined` if it doesn't exist.
   *
   * @param name - The state key
   * @returns The stored value, or `undefined` if not found
   *
   * @example
   * ```typescript
   * import { getState } from 'codemod:workflow';
   *
   * const files = getState<string[]>("migratedFiles");
   * if (files) {
   *   console.log(`Already migrated ${files.length} files`);
   * }
   * ```
   */
  export function getState<T>(name: string): T | undefined;

  /**
   * Removes a named state value.
   *
   * If the key was previously persisted, a removal diff will be generated
   * so the value is also removed from persistent storage.
   *
   * @param name - The state key to remove
   *
   * @example
   * ```typescript
   * import { unsetState } from 'codemod:workflow';
   *
   * unsetState("tempData");
   * ```
   */
  export function unsetState(name: string): void;

  /**
   * Acquires a named mutex lock for coordinating concurrent access to shared state.
   *
   * Returns a `release` function that must be called when done. The lock blocks other
   * threads trying to acquire the same named lock until it is released.
   *
   * @param name - The lock name
   * @returns A function that releases the lock (must be called exactly once)
   *
   * @example
   * ```typescript
   * import { acquireLock, getState, setState } from 'codemod:workflow';
   *
   * // Safely accumulate results from parallel file processing
   * const release = acquireLock("results");
   * try {
   *   const results = getState<string[]>("allResults") ?? [];
   *   results.push(currentFile);
   *   setState("allResults", results);
   * } finally {
   *   release();
   * }
   * ```
   */
  export function acquireLock(name: string): () => void;

  // ── Shard function types ────────────────────────────────────────────────

  /**
   * Input passed to a custom shard function by the engine.
   *
   * The engine handles all file discovery (globbing + optional codemod dry-run)
   * before calling the shard function. The function only needs to handle grouping.
   */
  export interface ShardInput {
    /** Relative paths of eligible files (pre-filtered by the engine) */
    files: string[];
    /** Absolute path to the target directory */
    targetDir: string;
    /** Previous shard assignments for incremental re-evaluation */
    previousShards: ShardResult[];
  }

  /**
   * A single shard result returned by a custom shard function.
   *
   * The `name` and `_meta_files` fields are required. Any additional
   * properties are exposed as `matrix.*` variables in downstream steps.
   *
   * Fields prefixed with `_meta_` are excluded from the matrix hash,
   * so re-indexing shards won't invalidate existing task identity.
   */
  export interface ShardResult {
    /** Shard identifier (used in PR titles, branch names) */
    name: string;
    /** Shard index — excluded from matrix hash for stability */
    _meta_shard: number;
    /** Files in this shard */
    _meta_files: string[];
    /** Arbitrary metadata exposed as matrix variables */
    [key: string]: unknown;
  }

  // ── Workflow authoring (workflow.ts) ───────────────────────────────────
  //
  // Everything below is used from a workflow file. `getState`, `setState`,
  // `unsetState` and `acquireLock` above also work inside function steps and
  // forEachFile callbacks; the step output functions only in JSSG transforms.
  //
  // Code inside `step(() => ...)` and `forEachFile(...)` callbacks runs in
  // its own isolate, with every module the run allows (`fs`, `fetch` and
  // `child_process` behind --allow-*). The rest of the workflow, the body,
  // runs again on every resume: there, `Date`, `Math.random` and
  // `crypto.randomUUID` are replay-safe, and modules that read files, the
  // network, the environment or the clock are build errors.

  import type { MetricEntry } from "codemod:metrics";

  /** A JSON value: what step results, captured variables and state may hold. */
  export type JsonValue =
    | string
    | number
    | boolean
    | null
    | readonly JsonValue[]
    | { readonly [key: string]: JsonValue };

  /** Stands in for a value that is not JSON, so the type error names the problem. */
  export interface NotJson<Reason extends string> {
    readonly __notJson: Reason;
  }

  /**
   * `T` with every part that is not JSON replaced by a `NotJson` marker,
   * so a value of type `T` is JSON when it is assignable to `JsonCompatible<T>`.
   */
  export type JsonCompatible<T> = unknown extends T
    ? T
    : T extends string | number | boolean | null | undefined | void
      ? T
      : T extends PromiseLike<unknown>
        ? NotJson<"a Promise: await it first">
        : T extends (...args: never[]) => unknown
          ? NotJson<"a function">
          : T extends ReadonlySet<unknown>
            ? NotJson<"a Set: use an array">
            : T extends ReadonlyMap<unknown, unknown>
              ? NotJson<"a Map: use a plain object">
              : T extends Date
                ? NotJson<"a Date: use date.toISOString()">
                : T extends RegExp
                  ? NotJson<"a RegExp: use its source string">
                  : T extends object
                    ? { [K in keyof T]: JsonCompatible<T[K]> }
                    : NotJson<"not JSON">;

  /** Step results are deeply frozen. */
  export type Frozen<T> = T extends (...args: never[]) => unknown
    ? T
    : T extends readonly (infer U)[]
      ? readonly Frozen<U>[]
      : T extends object
        ? { readonly [K in keyof T]: Frozen<T[K]> }
        : T;

  const taskResult: unique symbol;

  /** Work for `step()` to run, made by `forEachFile` or a task such as `jssg`. */
  export interface Task<Result> {
    readonly [taskResult]: Result;
  }

  /** What the workflow's default export receives. */
  export interface WorkflowContext<Params = Record<string, unknown>> {
    /** `--param key=value` values (JSON when they parse as JSON, otherwise strings). */
    params: Params;
    /** Absolute path of the directory the workflow runs on. */
    target: string;
    /** Whether this is a dry run: file changes are computed but not written. */
    dryRun: boolean;
  }

  /**
   * Runs a task as a durable step and resolves to its frozen result.
   *
   * The step starts when `step()` is called, so `Promise.all` runs steps in
   * parallel. Its result is recorded; on `codemod workflow resume`, finished
   * steps are replayed from the record instead of running again.
   *
   * @example
   * ```ts
   * const renamed = await step("Rename stylesheets", forEachFile("src/**\/*.less", (file) => {
   *   file.rename(file.path.replace(/\.less$/, ".css"));
   * }));
   * ```
   */
  export function step<Result>(task: Task<Result>): Promise<Frozen<Result>>;
  export function step<Result>(label: string, task: Task<Result>): Promise<Frozen<Result>>;
  /**
   * Runs a function once, in its own isolate, and records its result, which
   * must be JSON. Write it inline: outside variables arrive as frozen copies,
   * and assigning to them is a build error. Inside, `Date`, `fetch`, `fs` and
   * so on are real (as the run's capabilities allow).
   */
  export function step<Result>(
    run: () => PromiseLike<Checked<Result>>,
  ): Promise<Frozen<StepValue<Result>>>;
  export function step<Result>(
    label: string,
    run: () => PromiseLike<Checked<Result>>,
  ): Promise<Frozen<StepValue<Result>>>;
  export function step<Result>(run: () => Checked<Result>): Promise<Frozen<StepValue<Result>>>;
  export function step<Result>(
    label: string,
    run: () => Checked<Result>,
  ): Promise<Frozen<StepValue<Result>>>;

  /**
   * `T`, checked to be JSON: `Result` is inferred from the value, which must
   * then also be assignable to its JSON-compatible form.
   */
  export type Checked<T> = T & NoInfer<JsonCompatible<T>>;

  /** A function step that returns nothing records `null`. */
  export type StepValue<T> = [T] extends [void | undefined] ? null : Exclude<T, void | undefined>;

  /** One glob or several. */
  export type Globs = string | readonly string[];

  /** Languages `forEachFile` can select files by, through their extensions. */
  export type SelectorLanguage =
    | "typescript"
    | "tsx"
    | "javascript"
    | "css"
    | "json"
    | "html"
    | "python"
    | "rust"
    | "go"
    | "java"
    | "yaml";

  /** How a `forEachFile` step runs its callback. */
  export interface ForEachFileOptions {
    /** Files only these globs match are skipped. */
    exclude?: Globs;
    /** Only files whose text contains this string. */
    containing?: string;
    /**
     * Files each worker keeps in flight (default 1). Raise it for async
     * callbacks that wait on the network or `codemod:llm`.
     */
    concurrency?: number;
  }

  /**
   * Which files `forEachFile` visits, and how: a glob, a list of globs, or an
   * object. Without `include` or `language`, the files of the package's
   * `targets.languages` (codemod.yaml) are visited. `.gitignore` is respected.
   */
  export type FileSelector =
    | Globs
    | (ForEachFileOptions & {
        include: Globs;
        /** The default language for `parse(file)` and `parse(text)`. */
        language?: string;
      })
    | (ForEachFileOptions & {
        include?: undefined;
        /** Selects the language's file extensions, and is the default for `parse`. */
        language?: SelectorLanguage;
      });

  /** The file a `forEachFile` callback receives. Changes are applied when the step ends. */
  export interface WorkflowFile {
    /** Path relative to the target. */
    readonly path: string;
    /** Contents when the step started. */
    readonly text: string;
    /** The selector's `language`, which `parse(file)` uses; otherwise undefined. */
    readonly language?: string;
    /** Replace the contents. */
    write(text: string): void;
    /** Move the file to `path`, relative to the target. */
    rename(path: string): void;
    /** Remove the file. */
    delete(): void;
  }

  /** Files a step changed. */
  export interface FileChanges {
    written: string[];
    renamed: { from: string; to: string }[];
    deleted: string[];
  }

  export interface FileFailure {
    path: string;
    error: string;
  }

  export interface ForEachFileResult<Output> {
    /** What the callback returned, in path order. Files that returned nothing are left out. */
    outputs: Output[];
    changes: FileChanges;
    /** Files whose callback threw. */
    failures: FileFailure[];
    /** What callbacks recorded with `codemod:metrics`, by metric name. */
    metrics: Record<string, MetricEntry[]>;
  }

  /**
   * Runs `callback` once for every selected file, in parallel, in isolates.
   * The callback may be async. Returned values are collected into `outputs`.
   *
   * Write the callback inline. Variables from outside it arrive as frozen
   * copies and must be JSON; assigning to them is a build error. Imports
   * (`codemod:ast-grep`, `codemod:metrics`, packages, files next to the
   * workflow) are available inside.
   *
   * @example
   * ```ts
   * import { parse } from "codemod:ast-grep";
   *
   * const usages = await step(forEachFile({ language: "typescript", containing: "moment" }, (file) => {
   *   const found = parse(file).root().findAll({ rule: { pattern: 'import $X from "moment"' } });
   *   return found.length ? file.path : undefined;
   * }));
   * usages.outputs; // readonly string[]
   * ```
   */
  export function forEachFile<Output = undefined>(
    selector: FileSelector,
    callback: (file: WorkflowFile) => PromiseLike<Checked<Output>>,
  ): Task<ForEachFileResult<Exclude<Output, void | undefined>>>;
  export function forEachFile<Output = undefined>(
    selector: FileSelector,
    callback: (file: WorkflowFile) => Checked<Output>,
  ): Task<ForEachFileResult<Exclude<Output, void | undefined>>>;
  /** Visits the files of the package's `targets.languages` (codemod.yaml). */
  export function forEachFile<Output = undefined>(
    callback: (file: WorkflowFile) => PromiseLike<Checked<Output>>,
  ): Task<ForEachFileResult<Exclude<Output, void | undefined>>>;
  export function forEachFile<Output = undefined>(
    callback: (file: WorkflowFile) => Checked<Output>,
  ): Task<ForEachFileResult<Exclude<Output, void | undefined>>>;

  /**
   * Returns the whole workflow state as a frozen object.
   * (With a key, see the `getState(name)` overload above.)
   */
  export function getState(): Readonly<Record<string, JsonValue>>;

  /**
   * A result for tasks that do nothing in some runs: shell commands, agents
   * and skill installs in a dry run, git tasks without `--managed-git`.
   */
  export type Skippable<T> =
    | (T & { skipped?: false })
    | ({ skipped: true } & { [K in keyof T]?: undefined });

  export interface ShellOptions {
    /** Extra environment variables. */
    env?: Readonly<Record<string, string>>;
    /** Directory to run in, relative to the target (or absolute). Defaults to the current directory. */
    cwd?: string;
  }

  /**
   * Runs a shell command. It can read the workflow state from `$CODEMOD_STATE`
   * (a JSON file) or `$CODEMOD_STATE_<KEY>`, and set state by writing
   * `key=value` (or `key@=value` to append) lines to `$STATE_OUTPUTS`.
   */
  export function shell(
    command: string,
    options?: ShellOptions,
  ): Task<Skippable<{ output: string }>>;

  export type Capability = "fs" | "fetch" | "child_process";

  export interface JssgOptions {
    include?: Globs;
    exclude?: Globs;
    /** Directory under the target to restrict the run to. */
    basePath?: string;
    /** Process exactly these files, relative to the target. */
    files?: readonly string[];
    /** Defaults to `typescript`. */
    language?: string;
    /** Merged over the run's params; the transform reads them as `options.params`. */
    params?: Readonly<Record<string, unknown>>;
    /** The transform reads these as `options.matrixValues`. */
    matrix?: Readonly<Record<string, unknown>>;
    capabilities?: readonly Capability[];
    /**
     * Semantic analysis for `definition()` and `references()`: `"file"` (the default)
     * analyses each file on its own, `"workspace"` the whole target (or `root`), and
     * `"none"` turns it off.
     */
    semanticAnalysis?:
      | "file"
      | "workspace"
      | "none"
      | { mode: "file" | "workspace"; root?: string };
    threads?: number;
    /** Run the transform without applying its changes. */
    dryRun?: boolean;
  }

  export interface JssgResult {
    changes: FileChanges;
    unmodified: number;
    failures: FileFailure[];
  }

  /** Runs a JSSG transform file. `path` is relative to the workflow file. */
  export function jssg(path: string, options?: JssgOptions): Task<JssgResult>;

  export interface AstGrepOptions {
    include?: Globs;
    exclude?: Globs;
    basePath?: string;
    threads?: number;
  }

  export interface AstGrepResult {
    changes: FileChanges;
    matches: number;
    failures: FileFailure[];
  }

  /** Applies an ast-grep YAML rule file. `path` is relative to the workflow file. */
  export function astGrep(path: string, options?: AstGrepOptions): Task<AstGrepResult>;

  export type ShardMethod =
    | {
        type: "directory" | "codeowner";
        /** Target number of files per shard. */
        maxFilesPerShard: number;
        /** Trailing shards smaller than this are merged into the previous one. */
        minShardSize?: number;
      }
    | {
        /** A JS/TS file whose default export receives a `ShardInput` and returns `ShardResult[]`. */
        function: string;
      };

  export interface ShardOptions {
    method: ShardMethod;
    /** Directory to scan, relative to the target. */
    target?: string;
    /** Glob for eligible files when `jssg` is not set. */
    filePattern?: string;
    /** Keep only the files this transform would change. */
    jssg?: JssgOptions & { path: string };
    /** An earlier result of this step, so existing assignments are kept. */
    previous?: readonly ShardResult[];
  }

  /** Splits files into groups ("shards"), for example one per directory or code owner. */
  export function shard(options: ShardOptions): Task<ShardResult[]>;

  /** Runs another codemod package (registry name or local path) as part of this step. */
  export function codemod(
    source: string,
    options?: { params?: Readonly<Record<string, unknown>> },
  ): Task<{ output: JsonValue }>;

  /** Coding agents `agent(...)` can run, by id. Aliases such as `"claude"` work too. */
  export type CodingAgent =
    | "claude-code"
    | "codex"
    | "cursor"
    | "gemini"
    | "antigravity"
    | "opencode"
    | "copilot"
    | "amp"
    | "qwen-code"
    | "goose"
    | "droid"
    | "kimi"
    | "crush"
    | "openclaw"
    | "aider"
    | "kiro"
    | "auggie"
    | "cline";

  export interface AgentOptions {
    /**
     * The coding agent to run, which must be installed. Without one: the run's
     * agent (`--agent`, `LLM_AGENT`); the agent the workflow itself runs
     * inside, which gets the prompt back; one the person picks; or the
     * built-in agent when `LLM_API_KEY` is set.
     */
    agent?: CodingAgent | (string & {});
    /** Extra instructions for the agent. */
    systemPrompt?: string;
    /** A model name the agent understands. */
    model?: string;
    /**
     * `"full"` (default): edit files and run commands without asking.
     * `"read-only"`: the agent's read-only or planning mode where it has one.
     */
    autonomy?: "full" | "read-only";
    /** Stop the agent after this many seconds. */
    timeoutSeconds?: number;
    /** Built-in agent only: the most tool-using steps it may take. */
    maxSteps?: number;
    /** Built-in agent only. */
    tools?: readonly string[];
    /** Built-in agent only. */
    endpoint?: string;
    /** Built-in agent only. */
    apiKey?: string;
    /** Built-in agent only. */
    llmProtocol?: string;
  }

  export interface AgentUsage {
    inputTokens?: number;
    outputTokens?: number;
    costUsd?: number;
  }

  export interface AgentResult {
    skipped?: true;
    /** The prompt went back to the coding agent this workflow runs inside (`true`), or nowhere (`false`). */
    handedOff?: boolean;
    /** The agent that ran the prompt (`"built-in"` for the built-in agent). */
    agent?: string;
    /** The agent's final answer. */
    result?: string | null;
    /** The agent's session, to continue it there. */
    sessionId?: string | null;
    model?: string | null;
    /** Tokens and cost, as far as the agent reports them. */
    usage?: AgentUsage | null;
    /** Structured output from the built-in agent. */
    data?: JsonValue;
  }

  /** Hands a prompt to a coding agent. */
  export function agent(prompt: string, options?: AgentOptions): Task<AgentResult>;

  export interface InstallSkillOptions {
    /** Package to install, for example `@codemod/jest-to-vitest`. */
    package: string;
    /** Skill source path inside the package. */
    path?: string;
    harness?: "auto" | "claude" | "goose" | "opencode" | "cursor" | "codex" | "antigravity";
    scope?: "project" | "user";
    force?: boolean;
  }

  /** Installs a package's agent skill into a coding-agent harness. */
  export function installSkill(options: InstallSkillOptions): Task<Skippable<{ output: string }>>;

  /**
   * Waits for a person to approve. Interactive runs ask; with `--no-interactive`
   * the run pauses here until `codemod workflow resume --id <id> --approve`.
   */
  export function approval(message: string): Task<{ approved: true }>;

  /** Checks out a branch for the following changes. Only with `--managed-git`. */
  export function branch(name: string): Task<Skippable<{ branch: string }>>;

  /** Commits changes. Only with `--managed-git`. */
  export function commit(
    message: string,
    options?: { add?: readonly string[]; allowEmpty?: boolean },
  ): Task<Skippable<{ committed: boolean }>>;

  export interface PullRequestOptions {
    title: string;
    body?: string;
    draft?: boolean;
    base?: string;
    /** Defaults to the current branch. */
    branch?: string;
  }

  /** Pushes the branch and opens a pull request. Only with `--managed-git`. */
  export function pullRequest(
    options: PullRequestOptions,
  ): Task<Skippable<{ branch: string; url: string | null }>>;

  /** A finished step changed since the run was recorded, so resume cannot reuse its result. */
  export class NondeterminismError extends Error {}

  /** A step failed. */
  export class StepError extends Error {
    readonly label: string;
  }

  /** The run was canceled. */
  export class CanceledError extends Error {}

  /** The run is paused at an approval. */
  export class AwaitingApprovalError extends Error {}

  /**
   * Evaluates a workflow.yaml `${{ }}` expression.
   * @internal Emitted by `codemod workflow upgrade`; not for hand-written workflows.
   */
  export function __evaluate(
    kind: "condition" | "string" | "list",
    expression: string,
    scope: Readonly<Record<string, unknown>>,
  ): any;
}

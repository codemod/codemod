/**
 * In a workflow, as in Node.js: where the workflow file is, also inside its
 * steps and `forEachFile` callbacks.
 */
interface ImportMeta {
  /** The directory the workflow file is in. */
  dirname: string;
  /** The workflow file. */
  filename: string;
  /** The workflow file, as a `file:` URL. */
  url: string;
}

declare module "codemod:workflow" {
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
  // Everything below is used from a workflow file. Workflows have no state:
  // a step gets values from the body (a function step's captured variables,
  // shell `env`) and returns what it found on its result.
  //
  // Code inside `step(() => ...)` and `forEachFile(...)` callbacks runs in
  // its own isolate, with every module the run allows (`fs`, `fetch` and
  // `child_process` behind --allow-*). The rest of the workflow, the body,
  // runs again on every resume: there, `Date`, `Math.random` and
  // `crypto.randomUUID` are replay-safe, and modules that read files, the
  // network, the environment or the clock are build errors.

  /** A JSON value: what step results and captured variables may hold. */
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

  /** Work for `step()` to run, made by `forEachFile` or a task such as `shell`. */
  export interface Task<Result> {
    readonly [taskResult]: Result;
  }

  const astGrepHandler: unique symbol;

  /**
   * ast-grep rules, made by `astGrep(rules)`. Not a step on its own: give it
   * to `forEachFile` in place of a callback, to say which files it runs on.
   */
  export interface AstGrepHandler {
    readonly [astGrepHandler]: true;
  }

  /** What the workflow's default export receives. */
  export interface WorkflowContext<Params = Record<string, unknown>> {
    /** `--param key=value` values (JSON when they parse as JSON, otherwise strings). */
    params: Params;
    /** Absolute path of the directory the workflow runs on. */
    target: string;
    /** Whether this is a dry run: file changes are computed but not written. */
    dryRun: boolean;
    /** The platform running the workflow, e.g. `"win32"`, `"darwin"`, or `"linux"`. */
    readonly platform: string;
  }

  /**
   * A length of time: a number of milliseconds, or a number with a unit,
   * such as `"500ms"`, `"30s"`, `"10m"`, `"1h"` or `"2d"`.
   */
  export type Duration = number | `${number}${"ms" | "s" | "m" | "h" | "d"}`;

  /**
   * How a step is run. These do not identify the step: changing them does
   * not make a finished step run again on resume.
   */
  export interface StepOptions {
    /**
     * Runs the step again when it fails, up to this many more times. The
     * run sees only the last failure. Default: 0.
     */
    retries?: number;
    /** How long to wait before each retry. Default: `"1s"`. */
    retryDelay?: Duration;
    /**
     * Stops the step, and fails it, when one attempt takes longer than
     * this: commands are terminated and file changes not yet written are
     * dropped.
     */
    timeout?: Duration;
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
   * await step("Install", shell("pnpm install"), { retries: 2, timeout: "10m" });
   * ```
   */
  export function step<Result>(task: Task<Result>, options?: StepOptions): Promise<Frozen<Result>>;
  export function step<Result>(
    label: string,
    task: Task<Result>,
    options?: StepOptions,
  ): Promise<Frozen<Result>>;
  /**
   * Runs a function once, in its own isolate, and records its result, which
   * must be JSON. Write it inline: outside variables arrive as frozen copies,
   * and assigning to them is a build error. Inside, `Date`, `fetch`, `fs` and
   * so on are real (as the run's capabilities allow).
   */
  export function step<Result>(
    run: () => PromiseLike<Checked<Result>>,
    options?: StepOptions,
  ): Promise<Frozen<StepValue<Result>>>;
  export function step<Result>(
    label: string,
    run: () => PromiseLike<Checked<Result>>,
    options?: StepOptions,
  ): Promise<Frozen<StepValue<Result>>>;
  export function step<Result>(
    run: () => Checked<Result>,
    options?: StepOptions,
  ): Promise<Frozen<StepValue<Result>>>;
  export function step<Result>(
    label: string,
    run: () => Checked<Result>,
    options?: StepOptions,
  ): Promise<Frozen<StepValue<Result>>>;

  /**
   * Waits, as a step. The wait counts from when it began: a run that is
   * interrupted and resumed waits only for what is left. A dry run does not
   * wait.
   *
   * @example
   * ```ts
   * await sleep("30s");
   * ```
   */
  export function sleep(duration: Duration): Promise<null>;

  /**
   * Waits, as a step, for an event sent to the run with
   * `codemod workflow send <name> --id <run-id> [--data <json>]`, and
   * resolves to its data (`null` when none was given). An event sent before
   * the workflow gets here is kept for it. With `timeout`, the step fails if
   * no event comes in that time, counted from when the wait began.
   *
   * A dry run is not saved and cannot be sent events, so the step fails there.
   *
   * @example
   * ```ts
   * const ci = await waitForEvent<{ passed: boolean }>("ci-finished", { timeout: "1h" });
   * ```
   */
  export function waitForEvent<Data = JsonValue>(
    name: string,
    options?: { timeout?: Duration },
  ): Promise<Frozen<Data>>;

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
     * callbacks that wait on the network or `codemod/llm`.
     */
    concurrency?: number;
    /**
     * What `definition()` and `references()` on roots `parse(file)` makes can
     * find: `"file"` (the default) looks within each file, `"workspace"`
     * across the selected files of the target (or under `root`), and
     * `"none"` turns it off. Analysis runs only when they are called.
     */
    semanticAnalysis?: SemanticAnalysis;
    /**
     * Capabilities the callback needs besides the run's: `"fs"`, `"fetch"`,
     * `"child_process"`. Interactive runs ask for those the run does not
     * grant (`--allow-fs`, ..., or codemod.yaml); others go without. A dry
     * run never has `fs` or `child_process`.
     */
    capabilities?: readonly Capability[];
  }

  export type Capability = "fs" | "fetch" | "child_process";

  export type SemanticAnalysis =
    | "file"
    | "workspace"
    | "none"
    | { mode: "file" | "workspace"; root?: string };

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
    /**
     * Files whose callback threw. With some, the step is partial; when the
     * callback throws on every file, the step fails instead.
     */
    failures: FileFailure[];
  }

  /**
   * What `forEachFile(selector, callback, reducer, initial)` gives: the
   * callback's outputs folded into one value, in place of `outputs`.
   */
  export interface ReducedResult<Reduced> {
    /** What the reducer returned for the last output, or `initial` when no file returned one. */
    reduced: Reduced;
    changes: FileChanges;
    /**
     * Files whose callback threw; their outputs are not folded. With some,
     * the step is partial; when the callback throws on every file, the step
     * fails instead.
     */
    failures: FileFailure[];
  }

  /**
   * A `forEachFile` reducer: folds one file's output into the result so far,
   * as `Array#reduce`'s callback does. `path` is the file's path. Outputs
   * come in path order. It may be async.
   *
   * @example
   * ```ts
   * // scripts/by-package.ts
   * import type { FileReducer } from "codemod/workflow";
   *
   * const byPackage: FileReducer<Record<string, number>, number> = (counts, count, path) => {
   *   const name = path.split("/")[1] ?? ".";
   *   return { ...counts, [name]: (counts[name] ?? 0) + count };
   * };
   * export default byPackage;
   * ```
   */
  export type FileReducer<Reduced, Output> = (
    result: Reduced,
    output: Output,
    path: string,
  ) => Reduced | PromiseLike<Reduced>;

  /**
   * A `forEachFile` callback kept in its own file, as the file's default
   * export: annotate it with this type, then import it into the workflow
   * and give it to `forEachFile`. `Output` is what it returns for a file,
   * collected into the step's `outputs` (or folded by its reducer).
   *
   * @example
   * ```ts
   * // scripts/migrate.ts
   * import type { FileCallback } from "codemod/workflow";
   * import { parse } from "codemod/ast-grep";
   *
   * const migrate: FileCallback<{ path: string; count: number }> = (file) => {
   *   const root = parse(file).root();
   *   const found = root.findAll({ rule: { pattern: "var $A = $B" } });
   *   if (!found.length) return;
   *   file.write(root.commitEdits(found.map((node) => node.replace(node.text().replace(/^var/, "let")))));
   *   return { path: file.path, count: found.length };
   * };
   * export default migrate;
   *
   * // workflow.ts
   * import migrate from "./scripts/migrate.ts";
   * const result = await step(forEachFile({ language: "typescript" }, migrate));
   * ```
   */
  export type FileCallback<Output = void> = (
    file: WorkflowFile,
  ) => Output | undefined | void | PromiseLike<Output | undefined | void>;

  /**
   * Which files `forEachFile` applies `astGrep` rules to: a glob, a list of
   * globs, or an object. Without `include`, the files of the rules'
   * languages. Selected files of other languages are skipped: each rule
   * names its language. Options for callbacks (`language`,
   * `semanticAnalysis`, `capabilities`, `concurrency`) do not apply.
   */
  export type AstGrepSelector =
    | Globs
    | {
        include?: Globs;
        /** Files only these globs match are skipped. */
        exclude?: Globs;
        /** Only files whose text contains this string. */
        containing?: string;
      };

  /**
   * Applies ast-grep rules on every selected file, in the engine: no
   * JavaScript runs per file.
   *
   * @example
   * ```ts
   * import rules from "./rules/config.yml";
   *
   * const result = await step("Apply rules", forEachFile("src/**\/*.ts", astGrep(rules)));
   * result.matches; // number
   * ```
   */
  export function forEachFile(rules: AstGrepHandler): Task<AstGrepResult>;
  export function forEachFile(
    selector: AstGrepSelector,
    rules: AstGrepHandler,
  ): Task<AstGrepResult>;
  /** `astGrep(...)` returns nothing per file, so there is nothing to reduce. */
  export function forEachFile(
    selector: AstGrepSelector,
    rules: AstGrepHandler,
    reducer: AstGrepTakesNoReducer,
    initial?: unknown,
  ): never;

  /** What a reducer given with `astGrep(...)` is told it should be: it takes none. */
  export interface AstGrepTakesNoReducer {
    "astGrep(...) returns nothing per file, so there is nothing to reduce: give a reducer only with a callback": never;
  }

  /**
   * Runs `callback` once for every selected file, in parallel, in isolates.
   * The callback may be async. Returned values are collected into `outputs`.
   *
   * Write the callback inline, or import it from a file of the package
   * whose default export it is (`import migrate from "./scripts/migrate.ts"`);
   * the workflow body does not load that file, and the step runs again on
   * resume when the file, or a file it imports, changes. Variables from
   * outside an inline callback arrive as frozen copies and must be JSON;
   * assigning to them is a build error. Imports (`codemod/ast-grep`,
   * packages, files next to the workflow) are available inside.
   *
   * @example
   * ```ts
   * import { parse } from "codemod/ast-grep";
   *
   * const usages = await step(forEachFile({ language: "typescript", containing: "moment" }, (file) => {
   *   const found = parse(file).root().findAll({ rule: { pattern: 'import $X from "moment"' } });
   *   return found.length ? file.path : undefined;
   * }));
   * usages.outputs; // readonly string[]
   * ```
   */
  export function forEachFile<Returned>(
    selector: FileSelector,
    callback: (file: WorkflowFile) => Returned & NoInfer<CallbackReturn<Returned>>,
  ): Task<ForEachFileResult<Exclude<Awaited<Returned>, void | undefined>>>;
  /** Visits the files of the package's `targets.languages` (codemod.yaml). */
  export function forEachFile<Returned>(
    callback: (file: WorkflowFile) => Returned & NoInfer<CallbackReturn<Returned>>,
  ): Task<ForEachFileResult<Exclude<Awaited<Returned>, void | undefined>>>;
  /**
   * Runs `callback` on every selected file, as above, and folds what it
   * returns into one value with `reducer`, starting from `initial` (JSON),
   * as `Array#reduce` does. The step's result has that value as `reduced`,
   * in place of `outputs`: with many files, the outputs are never all held
   * at once.
   *
   * Each output is folded as its file finishes, in path order whatever
   * order files finish in, so the result is the same on every run. Files
   * that return nothing, or whose callback throws, are not folded. A
   * reducer that throws fails the step, and nothing the step's callbacks
   * changed is written. Write the reducer inline or import it from its
   * file, as callbacks are; it runs in an isolate of its own.
   *
   * @example
   * ```ts
   * const { reduced } = await step(forEachFile(
   *   "packages/*\/src/**\/*.ts",
   *   (file) => {
   *     const found = parse(file).root().findAll({ rule: { pattern: "var $A = $B" } });
   *     return found.length || undefined;
   *   },
   *   (counts, count, path) => {
   *     const name = path.split("/")[1];
   *     return { ...counts, [name]: (counts[name] ?? 0) + count };
   *   },
   *   {} as Record<string, number>,
   * ));
   * reduced; // Record<string, number>
   * ```
   */
  export function forEachFile<Returned, Reduced>(
    selector: FileSelector,
    callback: (file: WorkflowFile) => Returned & NoInfer<CallbackReturn<Returned>>,
    reducer: (
      result: Reduced,
      output: Exclude<Awaited<Returned>, void | undefined>,
      path: string,
    ) => ReducerReturn<Reduced>,
    initial: Reduced,
  ): Task<ReducedResult<Reduced>>;

  /**
   * What a `forEachFile` reducer may return: the result, as JSON, or a
   * promise of it. Anything else is marked with `NotJson`, as for callbacks.
   */
  export type ReducerReturn<Reduced> =
    | (Reduced & NoInfer<JsonCompatible<Reduced>>)
    | PromiseLike<Reduced & NoInfer<JsonCompatible<Reduced>>>;

  /**
   * What a `forEachFile` callback may return: JSON or nothing, or a promise
   * of either. Anything else is replaced by a `NotJson` marker that says why,
   * so the type error points at the returned value.
   */
  export type CallbackReturn<Returned> =
    Returned extends PromiseLike<infer Value>
      ? PromiseLike<JsonCompatible<Value>>
      : JsonCompatible<Returned>;

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
    /** Directory to run in, relative to the target (or absolute). Defaults to the target, or a file target's directory. */
    cwd?: string;
    /** Return the result of a command that exits with an error, instead of failing the step. */
    allowFailure?: boolean;
  }

  /** What a command printed, as text (the last 1 MiB of each), and how it exited. */
  export interface ShellResult {
    stdout: string;
    stderr: string;
    /** 0, unless `allowFailure` is set; -1 when a signal ended the command. */
    exitCode: number;
  }

  /** A value in a `shell` template: quoted as one word, or one word per item. */
  export type ShellValue = string | number | boolean | readonly (string | number | boolean)[];

  export type ShellTask = Task<Skippable<ShellResult>>;

  /**
   * Runs a shell command. It gets values through `env`; the workflow parses
   * what it needs from what the command prints. A command that exits with
   * an error fails the step, unless `allowFailure` is set.
   *
   * @example
   * await step("Install", shell("npm install"));
   * const { stdout } = await step("Detect", shell(`echo '{"major":19}'`));
   * JSON.parse(stdout); // { major: 19 }
   */
  export function shell(command: string, options?: ShellOptions): ShellTask;
  /**
   * Runs a shell command written as a template. Each value is quoted for the
   * shell, so a path with spaces stays one argument; the text around them is
   * taken as written, backslashes included.
   *
   * @example
   * await step("Find uses", shell`grep -rn ${name} src`);
   */
  export function shell(strings: TemplateStringsArray, ...values: ShellValue[]): ShellTask;
  /**
   * Options for a command written as a template.
   *
   * @example
   * await step("Build", shell({ cwd: "packages/app" })`npm run build`);
   */
  export function shell(
    options: ShellOptions,
  ): (strings: TemplateStringsArray, ...values: ShellValue[]) => ShellTask;

  /** What `forEachFile(selector, astGrep(rules))` gives. */
  export interface AstGrepResult {
    changes: FileChanges;
    /** Matches of all the rules, across the selected files. */
    matches: number;
    /** Files that could not be read or scanned. */
    failures: FileFailure[];
  }

  /**
   * What an ast-grep rule matches: a pattern, a node kind, a regex, how the
   * node relates to others, or a combination. See https://ast-grep.github.io/reference/rule.html
   */
  export interface AstGrepRuleMatcher {
    /** Code to match, with `$NAME` metavariables; or `{ context, selector, strictness }`. */
    pattern?:
      | string
      | {
          context: string;
          selector?: string;
          strictness?: "cst" | "smart" | "ast" | "relaxed" | "signature";
        };
    /** A tree-sitter node kind, such as `"call_expression"`. */
    kind?: string;
    /** A Rust regular expression the node's text must match. */
    regex?: string;
    /** Which nth child of its parent the node is. */
    nthChild?:
      | number
      | string
      | { position: number | string; reverse?: boolean; ofRule?: AstGrepRuleMatcher };
    /** The node's range in the file. */
    range?: {
      start: { line: number; column: number };
      end: { line: number; column: number };
    };
    /** The node is inside a node matching this. */
    inside?: AstGrepRelation;
    /** The node has a descendant matching this. */
    has?: AstGrepRelation;
    /** The node comes after a sibling matching this. */
    follows?: AstGrepRelation;
    /** The node comes before a sibling matching this. */
    precedes?: AstGrepRelation;
    /** Every rule matches. */
    all?: readonly AstGrepRuleMatcher[];
    /** At least one rule matches. */
    any?: readonly AstGrepRuleMatcher[];
    /** The rule does not match. */
    not?: AstGrepRuleMatcher;
    /** A utility rule, by id (from `utils`). */
    matches?: string;
  }

  /** A relational rule (`inside`, `has`, `follows`, `precedes`). */
  export interface AstGrepRelation extends AstGrepRuleMatcher {
    /** How far to look: to the end, or until a node matching this. */
    stopBy?: "neighbor" | "end" | AstGrepRuleMatcher;
    /** The field of the parent the node is in, such as `"body"`. */
    field?: string;
  }

  /** A rewrite of a metavariable's text, for `fix`. */
  export type AstGrepTransformation =
    | string
    | {
        substring?: { source: string; startChar?: number; endChar?: number };
        replace?: { source: string; replace: string; by: string };
        convert?: {
          source: string;
          toCase:
            | "lowerCase"
            | "upperCase"
            | "capitalize"
            | "camelCase"
            | "snakeCase"
            | "kebabCase"
            | "pascalCase";
          separatedBy?: readonly string[];
        };
        rewrite?: { source: string; rewriters: readonly string[]; joinBy?: string };
      };

  /**
   * An ast-grep rule: what an ast-grep YAML rule file holds, as an object
   * (one per document). See https://ast-grep.github.io/reference/yaml.html
   */
  export interface AstGrepRule {
    id: string;
    /** The language of the files the rule is for, such as `"typescript"`. */
    language: string;
    rule: AstGrepRuleMatcher;
    /** The replacement, with `$NAME` metavariables, or how to expand the replaced range. */
    fix?:
      | string
      | {
          template: string;
          expandStart?: AstGrepRelation;
          expandEnd?: AstGrepRelation;
        };
    /** Further rules each metavariable must match. */
    constraints?: Readonly<Record<string, AstGrepRuleMatcher>>;
    /** Rules other rules use with `matches`. */
    utils?: Readonly<Record<string, AstGrepRuleMatcher>>;
    /** New metavariables computed from others, for `fix`. */
    transform?: Readonly<Record<string, AstGrepTransformation>>;
    /** Rules `transform`'s `rewrite` applies. */
    rewriters?: readonly (Omit<AstGrepRule, "language"> & { language?: string })[];
    severity?: "hint" | "info" | "warning" | "error" | "off";
    message?: string;
    note?: string;
    url?: string;
    /** Globs of the files the rule is for. */
    files?: readonly string[];
    /** Globs of the files the rule is not for. */
    ignores?: readonly string[];
    labels?: Readonly<Record<string, { style: "primary" | "secondary"; message?: string }>>;
    metadata?: Readonly<Record<string, JsonValue>>;
  }

  /** One rule or several, as `astGrep` takes them and as an imported YAML file gives them. */
  export type AstGrepRules = AstGrepRule | readonly AstGrepRule[];

  /**
   * ast-grep rules for `forEachFile`: an object of the shape of an ast-grep
   * YAML rule (or an array of them), written in the workflow or built at run
   * time, or a YAML rule file imported as its content
   * (`import rules from "./rules/config.yml"`; several documents give an
   * array). It is not a step on its own: `forEachFile(selector,
   * astGrep(rules))` runs the rules on the selected files, in the engine.
   * The rules are part of the step: a changed rule runs the step again on
   * resume.
   *
   * @example
   * ```ts
   * import rules from "./rules/config.yml";
   *
   * await step(forEachFile(astGrep(rules)));
   * await step(forEachFile("src/**\/*.js", astGrep({
   *   id: "no-var",
   *   language: "javascript",
   *   rule: { pattern: "var $A = $B" },
   *   fix: "let $A = $B;",
   * })));
   * ```
   */
  export function astGrep(rules: AstGrepRules, options?: AstGrepOptions): AstGrepHandler;

  export interface AstGrepOptions {
    /**
     * Run the rules again on the text they fixed, until it no longer
     * changes: `true` makes up to 10 passes over each file, a number at most
     * that many. Of overlapping matches (`console.log(console.log(x))`) one
     * pass fixes the outer one, so the inner one needs another pass. Passes
     * run on the text in memory, so a dry run shows the same result. A file
     * still changing after the last pass is a failure and is left as it was
     * (a fix may match its own output).
     */
    repeat?: boolean | number;
  }

  export type ShardMethod =
    | {
        type: "directory" | "codeowner";
        /** Target number of files per shard. */
        maxFilesPerShard: number;
        /** Trailing shards smaller than this are merged into the previous one. */
        minShardSize?: number;
      }
    | {
        /**
         * A shard function: it receives a `ShardInput` and returns
         * `ShardResult[]`. Written inline, or imported from a file of the
         * package whose default export it is; it runs in its own isolate,
         * as callbacks do.
         */
        function: ShardFunction;
      };

  /** A shard function, written inline or as a file's default export. */
  export type ShardFunction = (
    input: ShardInput,
  ) => readonly ShardResult[] | PromiseLike<readonly ShardResult[]>;

  export interface ShardOptions {
    method: ShardMethod;
    /** Directory to scan, relative to the target. */
    target?: string;
    /**
     * Which files under `target` are eligible: a glob, a list of globs, or
     * `{ include, exclude, language }`. Without it, the files of the
     * package's `targets.languages`; built-in methods without `files` or
     * `filter` take every file under `target`.
     */
    files?: Globs | { include?: Globs; exclude?: Globs; language?: string };
    /**
     * Keeps the eligible files it returns `true` for, for example those the
     * codemod would change. A `forEachFile` callback, written inline or
     * imported, run on each file; what it does to the file is not kept.
     */
    filter?: (file: WorkflowFile) => boolean | undefined | PromiseLike<boolean | undefined>;
    /** An earlier result of this step, so existing assignments are kept. */
    previous?: readonly ShardResult[];
  }

  /**
   * Splits files into groups ("shards"), for example one per directory or
   * code owner.
   *
   * @example
   * ```ts
   * import wouldChange from "./scripts/would-change.ts";
   *
   * const shards = await step(shard({
   *   method: { type: "directory", maxFilesPerShard: 20 },
   *   target: "./src",
   *   files: { language: "tsx" },
   *   filter: wouldChange,
   * }));
   * ```
   */
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
     * `"read-only"`: the agent may look but not change anything. Agents that
     * cannot be held to that (see the agent table in the docs) fail the step
     * instead of running.
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

/**
 * A YAML file, imported as its content: `import rules from "./rules.yaml"`.
 * A file with several documents (`---`) gives an array of them. Typed for
 * ast-grep rule files, which is what workflows import YAML for.
 */
declare module "*.yaml" {
  const content: import("codemod:workflow").AstGrepRules;
  export default content;
}

declare module "*.yml" {
  const content: import("codemod:workflow").AstGrepRules;
  export default content;
}

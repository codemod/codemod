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
  // jssg `params`, shell `env`) and returns what it found on its result.
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

  const transformResult: unique symbol;

  /**
   * A transform for single files, made by `jssg` or `astGrep`. Give it to
   * `forEachFile` to say which files it runs on.
   */
  export interface Transform<Result> extends Task<Result> {
    readonly [transformResult]: Result;
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
    /**
     * Files whose callback threw. With some, the step is partial; when the
     * callback throws on every file, the step fails instead.
     */
    failures: FileFailure[];
  }

  /**
   * Runs `callback` once for every selected file, in parallel, in isolates.
   * The callback may be async. Returned values are collected into `outputs`.
   *
   * Write the callback inline. Variables from outside it arrive as frozen
   * copies and must be JSON; assigning to them is a build error. Imports
   * (`codemod/ast-grep`, packages, files next to the workflow) are
   * available inside.
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
   * Which files `forEachFile` runs a `jssg` transform on: a glob, a list of
   * globs, or an object. `language` is the language the files are parsed as
   * and, without `include`, selects that language's file extensions. Without
   * a `language`, the package's is used (`targets.languages` in codemod.yaml,
   * when it lists one), and otherwise `typescript`.
   */
  export type JssgSelector = Globs | { include?: Globs; exclude?: Globs; language?: string };

  /**
   * Which files `forEachFile` applies `astGrep` rules to: a glob, a list of
   * globs, or an object. Without `include`, the files of the rules' languages.
   */
  export type AstGrepSelector = Globs | { include?: Globs; exclude?: Globs };

  /**
   * Runs a JSSG transform, or applies an ast-grep rule file, on every
   * selected file. The selector's fields stand for the same options of
   * `jssg` and `astGrep`, so give each in one place.
   *
   * @example
   * ```ts
   * import codemod from "./scripts/codemod.ts";
   * import components from "./scripts/components.ts";
   * import rules from "./rules/config.yml";
   *
   * await step("Migrate imports", forEachFile(jssg(codemod)));
   * await step("Apply rules", forEachFile("src/**\/*.ts", astGrep(rules)));
   * await step("Migrate components", forEachFile(
   *   { include: "src/**\/*.tsx", exclude: "**\/*.test.tsx", language: "tsx" },
   *   jssg(components, { params: { strict: true } }),
   * ));
   * ```
   */
  export function forEachFile<Data>(transform: Transform<JssgResult<Data>>): Task<JssgResult<Data>>;
  export function forEachFile<Data>(
    selector: JssgSelector,
    transform: Transform<JssgResult<Data>>,
  ): Task<JssgResult<Data>>;
  export function forEachFile(transform: Transform<AstGrepResult>): Task<AstGrepResult>;
  export function forEachFile(
    selector: AstGrepSelector,
    transform: Transform<AstGrepResult>,
  ): Task<AstGrepResult>;

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

  /** One file's `data`, as the transform returned it. */
  export interface JssgData {
    file: string;
    data: JsonValue;
  }

  export interface JssgResult<Data = readonly JssgData[]> {
    changes: FileChanges;
    unmodified: number;
    failures: FileFailure[];
    /**
     * Each file's `data`, sorted by path, leaving out files that returned
     * none; or, with a reducer, what it folded them into.
     */
    data: Data;
  }

  /**
   * Folds each file's `data` into one value as the file finishes, so the
   * step never holds every file's data at once. Files arrive in no
   * particular order. Write `reduce` inline so the build can extract it.
   */
  export interface JssgReducer<T> {
    reduce: (result: T, entry: JssgData) => T | Promise<T>;
    initial: T;
  }

  /**
   * A JSSG transform: the default export of a transform file, imported into
   * the workflow (`import codemod from "./scripts/codemod.ts"`). The workflow
   * does not run it; the engine does, on each file.
   */
  export type JssgTransform = (...args: never[]) => unknown;

  /**
   * Runs an imported JSSG transform. Run it with
   * `forEachFile(selector, jssg(transform))`, which says which files it is
   * for. As a step on its own it runs on the files its options select.
   *
   * The transform is imported from a file of the package, by its default
   * export: the build passes the engine that file and includes it when
   * publishing. Only give the import to `jssg` (or `shard`); the workflow
   * does not load its code.
   *
   * @example
   * ```ts
   * import codemod from "./scripts/codemod.ts";
   *
   * await step("Migrate", forEachFile(jssg(codemod, { params: { strict: true } })));
   * ```
   */
  // `T` is unconstrained so `initial: 0` gives `number`, not `0`; the
  // result is checked to be JSON when the step runs.
  export function jssg<T>(
    transform: JssgTransform,
    options: JssgOptions & JssgReducer<T>,
  ): Transform<JssgResult<T>>;
  export function jssg(transform: JssgTransform, options?: JssgOptions): Transform<JssgResult>;

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
          expandStart?: AstGrepRuleMatcher;
          expandEnd?: AstGrepRuleMatcher;
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
   * Applies ast-grep rules: an object of the shape of an ast-grep YAML rule
   * (or an array of them), written in the workflow or built at run time, or
   * a YAML rule file imported as its content
   * (`import rules from "./rules/config.yml"`; several documents give an
   * array). Apply them with `forEachFile(selector, astGrep(rules))`, which
   * says which files they are for. As a step on its own it runs on the files
   * its options select. The rules are part of the step: a changed rule runs
   * the step again on resume.
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
  export function astGrep(rules: AstGrepRules, options?: AstGrepOptions): Transform<AstGrepResult>;

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
         * A shard function, imported from a file of the package by its
         * default export (`import shardFiles from "./scripts/shard.ts"`):
         * it receives a `ShardInput` and returns `ShardResult[]`.
         */
        function: ShardFunction;
      };

  /** The default export of a shard function file. */
  export type ShardFunction = (input: ShardInput) => unknown;

  export interface ShardOptions {
    method: ShardMethod;
    /** Directory to scan, relative to the target. */
    target?: string;
    /** Glob for eligible files when `jssg` is not set. */
    filePattern?: string;
    /** Keep only the files this imported transform would change. */
    jssg?: JssgOptions & { transform: JssgTransform };
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

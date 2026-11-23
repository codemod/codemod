// The workflow orchestrator: step(), replay, test plans and the task
// builders that make up `codemod/workflow`. It holds no host globals, so the
// same code runs in the engine's QuickJS context and in Node.js or Bun.
//
// `createRuntime(host)` returns the module's exports for one run, and what
// the host drives (`Runtime`): `replayNext()`, `close()` and `isClosed()`.
//
// This file is TypeScript that only needs its types removed. The engine
// strips them when it loads the file; the npm package builds it with tsc.

// ----------------------------------------------------------------- JSON ---

/** A JSON value: what steps record and tasks are described in. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

// ------------------------------------------------------- the run's records ---

/** `"partial"`: the step finished, but some of its files failed. */
export type StepStatus =
  | "succeeded"
  | "partial"
  | "failed"
  | "canceled"
  | "awaiting-approval"
  | "waiting";

/**
 * One step as the run store keeps it: `StepRecord` in
 * `crates/state/src/run_store.rs`, which reads and writes this JSON.
 */
export interface StepRecord {
  /** The step's place in the run's list of steps; it keeps it across resumes. */
  position: number;
  label: string;
  /** Canonical form of the step's label and task: what makes it this step. */
  key: string;
  status: StepStatus;
  result?: Json;
  error?: string | null;
  /** When a successful step finished, in milliseconds since the epoch. */
  finishedAt?: number | null;
  /** The order in which the run's steps finished. */
  seq?: number | null;
  /** The step did not run: a test gave its result. */
  given?: boolean;
  /** When a `sleep` or `waitForEvent` step began to wait. */
  since?: number | null;
  /** How many times the step ran, when retries made that more than once. */
  attempts?: number | null;
}

/** What a step is asked to do, as the host receives it: JSON with a `kind`. */
export type TaskContent = { kind: string } & Record<string, unknown>;

/** A function given to a host that runs callbacks in place. */
// eslint-disable-next-line typescript-eslint/no-explicit-any -- any function at all
export type Callback = (...args: any[]) => unknown;

/** `host.start(request)`: `HostRequest` in `crates/workflow/src/run.rs`. */
export interface StartRequest {
  position: number;
  label: string;
  task: TaskContent;
  /** The step's function, on hosts that run callbacks in place. */
  fn?: Callback;
  /** How long the task may run, from the step's `timeout`. */
  timeoutMs?: number;
  /** When the step began to wait (`sleep`, `waitForEvent`). */
  since?: number;
  /**
   * Other steps were still running when this one was issued. Managed git
   * steps share one checkout, so the engine refuses them then.
   */
  alongside?: boolean;
}

/** What a task resolves to: `TaskOutcome` in `crates/workflow/src/tasks/mod.rs`. */
export interface Outcome {
  result?: Json;
  /** Set when some of the task's files failed: how many, as a message. */
  partial?: string;
}

/** `host.event(event)`: `HostEvent` in `crates/workflow/src/run.rs`. */
export type StepEvent =
  | { type: "stepStarted" | "stepReplayed"; position: number; label: string }
  | {
      type: "stepRetrying";
      position: number;
      label: string;
      /** The attempt that failed, counting from 1. */
      attempt: number;
      attempts: number;
      error: string;
    };

/** How a test runs a workflow; steps are matched by label. */
export interface TestPlan {
  /** Stop after this step. */
  until?: string;
  /** Results for steps that should not run. */
  given?: Record<string, Json>;
}

/** What runs the orchestrator: the engine's QuickJS context, or Node.js. */
export interface Host {
  /** Steps recorded by earlier executions of the run. */
  history(): StepRecord[];
  /** Persist one step's outcome; `ms` is how long it took. */
  record(entry: StepRecord, ms: number): void;
  event(event: StepEvent): void;
  /** Whether a step about to start must not, because the run was asked to stop. */
  canceled(): boolean;
  /** Run a task. Rejects with `{ name, message }`. */
  start(request: StartRequest): Promise<Outcome>;
  /** Evaluate a workflow.yaml `${{ }}` expression. */
  evaluate(kind: string, expression: string, scope: unknown): unknown;
  /** The real time, for step records. */
  now(): number;
  /** Move the body's clock forward. */
  advanceClock(ms: number): void;
  /** `{ until?, given? }` when the run is a test. */
  testPlan(): TestPlan | null;
  /** The test's `until` step has started. */
  stop(): void;
  /** A replayed step waits for `replayNext()`. */
  wake?(): void;
  /**
   * Identifies a callback given as a function. Hosts without it need the
   * build to extract callbacks.
   */
  callbackId?(fn: Callback): string;
}

// --------------------------------------------------------------- errors ---

export class StepError extends Error {
  declare readonly label: string;

  constructor(label: string, cause: unknown) {
    super(`step '${label}' failed: ${messageOf(cause)}`);
    this.name = "StepError";
    this.label = label;
  }
}

export class CanceledError extends Error {
  constructor(message = "the run was canceled") {
    super(message);
    this.name = "CanceledError";
  }
}

export class AwaitingApprovalError extends Error {
  constructor(message?: string) {
    super(message);
    this.name = "AwaitingApprovalError";
  }
}

// ---------------------------------------------------------------- tasks ---

const TASK = Symbol.for("codemod.workflow.task");

/** The files a `forEachFile` step visits, as the engine receives it. */
export interface Selector {
  include: string[];
  exclude: string[];
  language?: string;
  containing?: string;
  concurrency?: number;
  semanticAnalysis?: Json;
  capabilities?: string[];
}

/** A callback the build extracted, or one given as a function. */
export interface Extracted {
  id: string;
  /** The outside values an extracted callback uses, read when its step is called. */
  captures?: () => Json;
  fn?: Callback;
}

/** What the build puts in place of an extracted callback. */
export interface CallbackReference {
  __flowCallback: string;
  captures: () => Json;
  /** On hosts that run callbacks in place: the function itself. */
  fn?: Callback;
}

export interface ForEachFileTask {
  readonly [TASK]: "forEachFile";
  readonly selector: Selector;
  readonly callback: Extracted;
}

export interface HostTask {
  readonly [TASK]: "host";
  readonly content: TaskContent;
  /**
   * Functions the engine runs in isolates, by the field of `content` they
   * go in (`shard`'s `method.function` and `filter`); their captures are
   * read when the step starts.
   */
  readonly callbacks?: Readonly<Record<string, Extracted>>;
}

export interface DependencyGraphTask {
  readonly [TASK]: "dependencyGraph";
  readonly lockfiles: string[] | undefined;
  readonly callback: Extracted;
}

export type Task = ForEachFileTask | DependencyGraphTask | HostTask;

const HANDLER = Symbol.for("codemod.workflow.handler");

/**
 * What `astGrep(rules)` makes: not a task, but what `forEachFile` runs on
 * each selected file in place of a callback.
 */
export interface AstGrepHandler {
  readonly [HANDLER]: "astGrep";
  readonly rules: Json[];
  /** Passes over each file at most; more than one repeats until nothing changes. */
  readonly repeat?: number;
}

/** Passes `astGrep(rules, { repeat: true })` makes over a file at most. */
const REPEAT_PASSES = 10;

/** A length of time: milliseconds, or a string such as "30s" or "10m". */
export type Duration = number | string;

/** `step(..., options)`: how to run a step. */
export interface StepOptions {
  retries?: number;
  retryDelay?: Duration;
  timeout?: Duration;
}

/**
 * The task builders (`forEachFile`, `shell`, `astGrep`, ...). They only describe
 * work, so they need no run. `options.callbackId(fn)` lets callbacks be given
 * as functions, for hosts that run them in place.
 */
export function createTasks(options: Pick<Host, "callbackId"> = {}) {
  /** Inserted by the build in place of an extracted callback. */
  function __callback(id: string, captures: () => Json): CallbackReference {
    return { __flowCallback: id, captures };
  }

  /**
   * Inserted by the Node.js host's build in place of an imported
   * `forEachFile` callback: the function, which runs in place, with the id
   * the build gave it, which changes with its file and the files it imports.
   */
  function __importedCallback(id: string, fn: Callback): CallbackReference {
    return { __flowCallback: id, captures: () => ({}), fn };
  }

  /** A callback the build extracted, or one given as a function on hosts that run them in place. */
  function callbackOf(value: unknown): Extracted | null {
    if (isRecord(value) && typeof value.__flowCallback === "string") {
      const fn = typeof value.fn === "function" ? (value.fn as Callback) : undefined;
      return {
        id: value.__flowCallback,
        captures: value.captures as Extracted["captures"],
        ...(fn ? { fn } : {}),
      };
    }
    if (typeof value === "function" && typeof options.callbackId === "function") {
      const fn = value as Callback;
      return { id: options.callbackId(fn), fn };
    }
    return null;
  }

  /** Run a callback, or `astGrep(...)` rules, on every selected file. */
  function forEachFile(selector: unknown, callback?: unknown): ForEachFileTask | HostTask {
    if (callback === undefined && (callbackOf(selector) || isHandler(selector))) {
      callback = selector;
      selector = undefined;
    }
    if (isHandler(callback)) return astGrepFiles(selector, callback);
    const extracted = callbackOf(callback);
    if (!extracted) {
      throw new TypeError(
        typeof callback === "function"
          ? "forEachFile: the callback was not extracted by the build; run the workflow through the codemod-workflow build"
          : "forEachFile takes a callback, written inline as a function or imported from its file, or astGrep(...)",
      );
    }
    return Object.freeze({
      [TASK]: "forEachFile" as const,
      selector: normalizeSelector(selector),
      callback: extracted,
    });
  }

  /**
   * `forEachFile(selector, astGrep(rules))`: the engine's ast-grep task,
   * which applies the rules to the selected files without running any
   * JavaScript per file. The selector picks files with `include`, `exclude`
   * and `containing`; the rules name their languages, and without `include`
   * the files of those languages are selected.
   */
  function astGrepFiles(selector: unknown, handler: AstGrepHandler): HostTask {
    const { include, exclude, language, containing, ...forCallbacks } = normalizeSelector(selector);
    const field = Object.keys(forCallbacks)[0];
    if (field !== undefined) {
      throw new TypeError(`forEachFile: selector.${field} is for callbacks, not for astGrep(...)`);
    }
    if (language !== undefined) {
      throw new TypeError(
        "forEachFile: the rules of astGrep(...) name their languages; select files with globs",
      );
    }
    return hostTask("astGrep", {
      rules: handler.rules,
      repeat: handler.repeat,
      include: include.length ? include : undefined,
      exclude: exclude.length ? exclude : undefined,
      containing,
    });
  }

  /**
   * Read the target's lockfiles into one dependency graph and run `callback`
   * with a live handle on it, in its own isolate. Only what the callback
   * returns is recorded.
   */
  function dependencyGraph(options: unknown, callback: unknown): DependencyGraphTask {
    const extracted = callbackOf(callback);
    if (!extracted || extracted.fn) {
      throw new TypeError(
        "dependencyGraph: the callback was not extracted by the build; run the workflow through the codemod-workflow build",
      );
    }
    const { lockfiles, ...rest } = isRecord(options) ? options : {};
    const unknown = Object.keys(rest);
    if (unknown.length) throw new TypeError(`dependencyGraph: unknown option '${unknown[0]}'`);
    return Object.freeze({
      [TASK]: "dependencyGraph" as const,
      lockfiles:
        lockfiles === undefined ? undefined : globs(lockfiles, "lockfiles", "dependencyGraph: "),
      callback: extracted,
    });
  }

  /**
   * Run a shell command: `shell(command, options?)`, or as a tagged
   * template, ``shell`grep -r ${pattern} src` ``, where each value is quoted
   * for the shell. ``shell(options)`...` `` gives the template options.
   */
  function shell(command: unknown, ...rest: unknown[]): HostTask | ShellTag {
    if (isTemplateStrings(command)) return shellTemplate({}, command, rest);
    if (typeof command === "string") {
      return hostTask("shell", { ...(rest[0] as object | undefined), command });
    }
    if (isRecord(command) && rest.length === 0) {
      const options = command;
      return (strings: TemplateStringsArray, ...values: unknown[]) => {
        if (!isTemplateStrings(strings)) {
          throw new TypeError("shell(options) is used as a tag: shell(options)`command`");
        }
        return shellTemplate(options, strings, values);
      };
    }
    throw new TypeError("shell: expected a command, shell`command`, or shell(options)`command`");
  }

  function shellTemplate(
    options: object,
    strings: TemplateStringsArray,
    values: unknown[],
  ): HostTask {
    // The text as written, backslashes included: it is shell, not JavaScript.
    let command = strings.raw[0] ?? "";
    values.forEach((value, index) => {
      command += shellQuote(value, index) + (strings.raw[index + 1] ?? "");
    });
    return hostTask("shell", { ...options, command });
  }

  /**
   * ast-grep rules for `forEachFile`: a rule object of the shape of an
   * ast-grep YAML rule, or an array of them, written in the workflow, built
   * at run time, or imported from a YAML file. Not a step on its own:
   * `forEachFile(selector, astGrep(rules))` says which files they run on.
   * The rules are part of the step's content, so a changed rule runs the
   * step again on resume.
   */
  function astGrep(rules: unknown, options?: unknown): AstGrepHandler {
    const list = ruleList(rules);
    if (options === undefined) return Object.freeze({ [HANDLER]: "astGrep" as const, rules: list });
    if (!isRecord(options) || Array.isArray(options)) {
      throw new TypeError("astGrep's options must be an object: astGrep(rules, { repeat: true })");
    }
    for (const key of Object.keys(options)) {
      if (key !== "repeat") {
        throw new TypeError(
          ["include", "exclude", "containing", "language", "basePath"].includes(key)
            ? `astGrep: '${key}' goes in forEachFile's selector: forEachFile({ ${key} }, astGrep(rules))`
            : `astGrep: unknown option '${key}' (it takes repeat)`,
        );
      }
    }
    const { repeat } = options;
    let passes = 1;
    if (repeat === true) passes = REPEAT_PASSES;
    else if (
      typeof repeat === "number" &&
      Number.isInteger(repeat) &&
      repeat >= 1 &&
      repeat <= 1000
    ) {
      passes = repeat;
    } else if (repeat !== false && repeat !== undefined) {
      throw new TypeError(
        "astGrep: repeat must be true (up to 10 passes) or the most passes to make, a whole number from 1 to 1000",
      );
    }
    return Object.freeze({
      [HANDLER]: "astGrep" as const,
      rules: list,
      ...(passes > 1 ? { repeat: passes } : {}),
    });
  }

  /**
   * Split files into shards: `shard({ method, target, files, filter,
   * previous })`. `method` is built in (`{ type, maxFilesPerShard }`) or a
   * function (`{ function }`), and `filter` a per-file callback; both are
   * written inline or imported, and run in isolates, as callbacks do.
   */
  function shard(options: unknown): HostTask {
    if (!isRecord(options)) throw new TypeError("shard takes options: shard({ method, ... })");
    const known = new Set(["method", "target", "files", "filter", "previous"]);
    for (const key of Object.keys(options)) {
      if (key === "jssg" || key === "filePattern") {
        throw new TypeError(
          key === "jssg"
            ? "shard's jssg was removed: give a filter, filter: (file) => ..., that keeps the files it returns true for"
            : 'shard\'s filePattern was removed: select files with files: "**/*.ts" (or a selector object)',
        );
      }
      if (!known.has(key)) throw new TypeError(`shard: unknown option '${key}'`);
    }
    const { method, files, filter, ...rest } = options;
    const callbacks: Record<string, Extracted> = {};
    const extracted = (value: unknown, what: string): Extracted => {
      const callback = callbackOf(value);
      if (!callback || callback.fn) {
        throw new TypeError(
          typeof value === "function"
            ? `${what} was not extracted by the build: run the workflow with the codemod CLI, or with runWorkflow(path) from codemod/host or codemod/testing`
            : `${what} takes a function, written inline or imported from its file`,
        );
      }
      return callback;
    };
    if (!isRecord(method)) {
      throw new TypeError(
        "shard: method must be { type, maxFilesPerShard } or { function }, written inline",
      );
    }
    let content: Record<string, unknown> = { ...rest, method };
    if ("function" in method) {
      callbacks["method.function"] = extracted(method.function, "shard's method.function");
      const { function: _, ...others } = method;
      content.method = others;
    }
    if (filter !== undefined) callbacks.filter = extracted(filter, "shard's filter");
    if (files !== undefined) {
      const { include, exclude, language, ...forCallbacks } = normalizeSelector(files);
      const field = Object.keys(forCallbacks)[0];
      if (field !== undefined) throw new TypeError(`shard: files.${field} is for forEachFile`);
      content = { ...content, files: withoutUndefined({ include, exclude, language }) };
    }
    const task = hostTask("shard", content);
    return Object.freeze({ ...task, callbacks });
  }

  /** Run another codemod package (registry name or local path). */
  function codemod(source: string, options: object = {}): HostTask {
    return hostTask("codemod", { ...options, source });
  }

  /** Hand a prompt to a coding agent. */
  function agent(prompt: string, options: object = {}): HostTask {
    return hostTask("agent", { ...options, prompt });
  }

  /** Install an agent skill package. */
  function installSkill(options: object): HostTask {
    return hostTask("installSkill", options);
  }

  /** Wait for a person to approve before continuing. */
  function approval(message: string): HostTask {
    return hostTask("approval", { message });
  }

  /** Wait for a time: milliseconds, or a string such as "30s" or "10m". */
  function sleep(duration: Duration): HostTask {
    return hostTask("sleep", { duration: milliseconds(duration, "sleep") });
  }

  /** Wait for an event sent to the run (`codemod workflow send`). */
  function waitForEvent(name: string, options: { timeout?: Duration } = {}): HostTask {
    if (typeof name !== "string" || !/^[A-Za-z0-9._-]{1,100}$/.test(name)) {
      throw new TypeError(
        "waitForEvent: the event name must be 1 to 100 letters, digits, '.', '_' or '-'",
      );
    }
    onlyKeys(options, ["timeout"], "waitForEvent");
    return hostTask("waitForEvent", {
      name,
      timeout:
        options.timeout === undefined
          ? undefined
          : milliseconds(options.timeout, "waitForEvent: timeout"),
    });
  }

  return {
    callbackOf,
    __callback,
    __importedCallback,
    forEachFile,
    dependencyGraph,
    shell,
    astGrep,
    shard,
    codemod,
    agent,
    installSkill,
    approval,
    sleep,
    waitForEvent,
  };
}

function hostTask(kind: string, options: unknown): HostTask {
  const content = withoutUndefined(options);
  checkJson(content, kind);
  return Object.freeze({
    [TASK]: "host" as const,
    content: { ...(content as Record<string, unknown>), kind },
  });
}

// ------------------------------------------------------------------ git ---

/**
 * The git steps, `codemod/git`. They describe tasks like the rest; `step`
 * runs them. Not part of `createTasks`, so `codemod/workflow` doesn't export
 * them.
 */
export const gitTasks = Object.freeze({
  /** Check out a branch for the following changes (managed git only). */
  branch(name: string): HostTask {
    return hostTask("branch", { name });
  },
  /** Commit changes (managed git only). */
  commit(message: string, options: object = {}): HostTask {
    return hostTask("commit", { ...options, message });
  },
  /** Push the branch and open a pull request (managed git only). */
  pullRequest(options: object): HostTask {
    return hostTask("pullRequest", options);
  },
});

/** `astGrep`'s rules as a list of JSON objects, or why they are not. */
function ruleList(value: unknown): Json[] {
  if (typeof value === "string") {
    const specifier = /^\.\.?\//.test(value) ? value : `./${value}`;
    throw new TypeError(
      `astGrep takes rules, not a file path: import rules from ${JSON.stringify(specifier)}, then astGrep(rules)`,
    );
  }
  const list: unknown[] = Array.isArray(value) ? value : [value];
  if (list.length === 0) throw new TypeError("astGrep needs at least one rule");
  return list.map((rule, index) => {
    const what = Array.isArray(value) ? `astGrep: rule ${index}` : "astGrep: the rule";
    if (!isRecord(rule) || Array.isArray(rule)) {
      throw new TypeError(
        `${what} must be an object of the shape of an ast-grep YAML rule ({ id, language, rule, fix, ... })`,
      );
    }
    // A field left undefined is left out, as in JSON.stringify; anything
    // else that is not JSON (a function, a RegExp, a Map) is an error.
    const cleaned = withoutUndefined(rule);
    checkJson(cleaned, what);
    return cleaned;
  });
}

/** Tasks that wait: their step records when the wait began. */
const WAITS = new Set(["sleep", "waitForEvent"]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object";
}

function onlyKeys(options: unknown, known: readonly string[], what: string): void {
  if (!isRecord(options) || Array.isArray(options)) {
    throw new TypeError(`${what}: options must be an object`);
  }
  for (const key of Object.keys(options)) {
    if (!known.includes(key)) {
      throw new TypeError(`${what}: unknown option '${key}'; expected ${known.join(", ")}`);
    }
  }
}

const UNITS = { ms: 1, s: 1000, m: 60_000, h: 3_600_000, d: 86_400_000 } as const;

/** A duration in milliseconds, from a number of them or a string such as "30s". */
export function milliseconds(value: unknown, what: string): number {
  if (typeof value === "number" && Number.isFinite(value) && value >= 0) return Math.round(value);
  const match =
    typeof value === "string" ? /^(\d+(?:\.\d+)?)\s*(ms|s|m|h|d)$/.exec(value.trim()) : null;
  if (!match) {
    throw new TypeError(
      `${what}: expected milliseconds or a duration such as "500ms", "30s", "10m", "1h" or "2d"`,
    );
  }
  return Math.round(Number(match[1]) * UNITS[match[2] as keyof typeof UNITS]);
}

/** A duration in its largest whole unit, as the engine words it. */
export function describeDuration(ms: number): string {
  for (const unit of ["h", "m", "s"] as const) {
    if (ms >= UNITS[unit] && ms % UNITS[unit] === 0) return `${ms / UNITS[unit]}${unit}`;
  }
  return `${ms}ms`;
}

/** How often to try a step, and for how long. */
interface Policy {
  attempts: number;
  /** Milliseconds between attempts. */
  retryDelay: number;
  /** Milliseconds one attempt may take. */
  timeout: number | undefined;
}

/** What `step(..., options)` asks for. */
function policyOf(options: unknown, what: string): Policy {
  if (options === undefined) return { attempts: 1, retryDelay: 0, timeout: undefined };
  if (isRecord(options) && "state" in options) {
    throw new TypeError(
      `${what}: steps take no \`state\`: give values to the task itself (a function step's or callback's variables, shell \`env\`) and read what it found from its result`,
    );
  }
  onlyKeys(options, ["retries", "retryDelay", "timeout"], what);
  const { retries = 0, retryDelay = "1s", timeout } = options as StepOptions;
  if (!Number.isInteger(retries) || retries < 0) {
    throw new TypeError(`${what}: retries must be a whole number, 0 or more`);
  }
  const limit = timeout === undefined ? undefined : milliseconds(timeout, `${what}: timeout`);
  if (limit === 0) throw new TypeError(`${what}: timeout must be more than 0`);
  return {
    attempts: retries + 1,
    retryDelay: milliseconds(retryDelay, `${what}: retryDelay`),
    timeout: limit,
  };
}

// -------------------------------------------------------------- runtime ---

/** A recorded step the body has issued again, waiting to settle. */
interface Replay {
  order: number;
  settle(): void;
}

/** What `createRuntime` returns: `codemod/workflow` for one run, and what its host drives. */
export type Runtime = ReturnType<typeof createRuntime>;

/** A tag made by `shell(options)`. */
type ShellTag = (strings: TemplateStringsArray, ...values: unknown[]) => HostTask;

function isTemplateStrings(value: unknown): value is TemplateStringsArray {
  return Array.isArray(value) && Array.isArray((value as { raw?: unknown }).raw);
}

/**
 * A value in a `shell` template, quoted for the shell: one word whatever it
 * holds. An array is one word per item.
 */
function shellQuote(value: unknown, index: number): string {
  if (Array.isArray(value)) return value.map((item) => shellQuote(item, index)).join(" ");
  if (typeof value !== "string" && typeof value !== "number" && typeof value !== "boolean") {
    throw new TypeError(
      `shell: value ${index + 1} in the command is ${value === null ? "null" : typeof value}; give a string, a number, a boolean, or an array of them`,
    );
  }
  const text = String(value);
  if (/^[\w@%+=:,./-]+$/.test(text)) return text;
  return `'${text.replaceAll("'", `'\\''`)}'`;
}

export function createRuntime(host: Host) {
  const tasks = createTasks(host);
  const { callbackOf } = tasks;

  // What earlier executions of the run recorded. A step is the one recorded
  // when it has the same label and content (its key), wherever and whenever
  // the body issues it: the order steps are issued in depends on how fast
  // other steps finish, so it cannot say which step is which. Steps that are
  // the same in both are interchangeable and take the records in turn.
  const recorded = new Map<string, StepRecord[]>();
  // Recorded steps that did not finish, for a changed step to take over.
  const unfinished = new Set<StepRecord>();
  // A step keeps its position for the whole run; new steps go after.
  let nextPosition = 0;
  // Steps are numbered in the order they finish, across executions of the run.
  let nextSeq = 0;
  for (const entry of [...host.history()].sort((a, b) => a.position - b.position)) {
    const same = recorded.get(entry.key) ?? [];
    same.push(entry);
    recorded.set(entry.key, same);
    if (!finished(entry)) unfinished.add(entry);
    nextPosition = Math.max(nextPosition, entry.position + 1);
    if (typeof entry.seq === "number") nextSeq = Math.max(nextSeq, entry.seq + 1);
  }

  /**
   * The recorded step this one is, or the position of an unfinished step it
   * replaces: a step with a recorded name but new content is the failed step,
   * fixed. A finished step is never replaced; its record stays for a step
   * with its content.
   */
  function claim(
    label: string | undefined,
    key: string,
    content: TaskContent,
  ): { position: number; entry?: StepRecord } {
    const entry = recorded.get(key)?.shift();
    if (entry) {
      unfinished.delete(entry);
      return { position: entry.position, entry };
    }
    for (const entry of unfinished) {
      if (entry.label !== (label ?? defaultLabel(entry.position, content))) continue;
      unfinished.delete(entry);
      const others = recorded.get(entry.key);
      others?.splice(others.indexOf(entry), 1);
      return { position: entry.position };
    }
    return { position: nextPosition++ };
  }

  // A test run can stop after one step (`until`) and skip steps with a given
  // result (`given`), both matched by label.
  const plan: TestPlan = host.testPlan() ?? {};
  const given = new Map<string, Json>(Object.entries(plan.given ?? {}));

  // Once closed, no step starts: steps issued from then on never settle. A
  // run closes when a test's `until` step starts, when a step is canceled or
  // waits for an approval, and when the host ends the run.
  let closed = false;

  // Recorded steps the body has issued again, waiting to settle. They settle
  // one at a time in the order they finished when they ran, so the body sees
  // results, the clock and promise races as it did then.
  const replaying: Replay[] = [];

  // Steps issued and not yet settled. A step issued while this is above
  // zero runs alongside others (`StartRequest.alongside`).
  let running = 0;

  /**
   * Settle the next replayed step. The host calls it whenever the workflow
   * has nothing left to run, until it returns false.
   */
  function replayNext(): boolean {
    if (closed) return false;
    let next: Replay | undefined;
    for (const replay of replaying) {
      if (!next || replay.order < next.order) next = replay;
    }
    if (!next) return false;
    replaying.splice(replaying.indexOf(next), 1);
    next.settle();
    return true;
  }

  /** The run is ending: no step starts from here on. */
  function close(): void {
    closed = true;
  }

  /** Whether steps issued now would never start. */
  function isClosed(): boolean {
    return closed;
  }

  // The body's clock only moves when a step finishes, to the time recorded
  // for it, so Date.now() in the body gives the same answer on every replay.
  function settleClock(entry: Pick<StepRecord, "finishedAt">): void {
    if (typeof entry.finishedAt === "number") host.advanceClock(entry.finishedAt);
  }

  /** Evaluate a workflow.yaml `${{ }}` expression; used by translated workflows. */
  function __evaluate(kind: string, expression: string, scope: unknown): unknown {
    return host.evaluate(kind, expression, scope);
  }

  // ----------------------------------------------------------------- step ---

  /**
   * Run a task as a durable step: `step(label?, task, options?)`. Starts
   * immediately; the returned promise resolves to the step's frozen result.
   * `options` may give `retries`, `retryDelay` and `timeout`; they say how to
   * run the step, not what it is, so changing them does not make it another
   * step.
   */
  function step(...args: unknown[]): Promise<Json> {
    const label = typeof args[0] === "string" ? (args.shift() as string) : undefined;
    const alongside = running > 0;
    running++;
    return runStep(label, args[0], args[1], alongside)
      .finally(() => running--)
      .then(freeze);
  }

  /** Wait for a time, as a step: a resumed run waits for what is left. */
  function sleep(duration: Duration): Promise<Json> {
    return step(tasks.sleep(duration));
  }

  /** Wait for an event sent to the run, as a step; resolves to the event's data. */
  function waitForEvent(name: string, options?: { timeout?: Duration }): Promise<Json> {
    return step(tasks.waitForEvent(name, options));
  }

  async function runStep(
    label: string | undefined,
    task: unknown,
    options: unknown,
    alongside: boolean,
  ): Promise<Json> {
    const described = label === undefined ? "step" : `step '${label}'`;
    const policy = policyOf(options, described);
    // Everything up to the first await runs synchronously inside step(), so
    // captures are a snapshot taken when the step is called.
    let content: TaskContent;
    let fn: Callback | undefined;
    const callback = callbackOf(task);
    if (callback) {
      content = { kind: "function", callback: callback.id, captures: capturesOf(callback) };
      fn = callback.fn;
    } else if (typeof task === "function") {
      throw new TypeError(
        `${described}: write the function inline, as step(() => ...), so the build can run it in its own isolate`,
      );
    } else if (isTask(task) && task[TASK] === "forEachFile") {
      content = {
        kind: "forEachFile",
        selector: task.selector,
        callback: task.callback.id,
        captures: capturesOf(task.callback),
      };
      fn = task.callback.fn;
    } else if (isTask(task) && task[TASK] === "dependencyGraph") {
      content = withoutUndefined({
        kind: "dependencyGraph",
        lockfiles: task.lockfiles,
        callback: task.callback.id,
        captures: capturesOf(task.callback),
      }) as TaskContent;
    } else if (isTask(task) && task[TASK] === "host") {
      content = task.content;
      for (const [field, callback] of Object.entries(task.callbacks ?? {})) {
        content = withField(content, field, {
          callback: callback.id,
          captures: capturesOf(callback) ?? {},
        });
      }
    } else if (isHandler(task)) {
      throw new TypeError(
        `${described}: astGrep(...) is not a step on its own; run it on files with step(forEachFile(selector, astGrep(rules)))`,
      );
    } else {
      throw new TypeError(`${described}: expected a task such as forEachFile(...) or a function`);
    }

    const key = canonical([label ?? null, content]);
    if (closed) return new Promise<Json>(() => {});
    const { position, entry: prior } = claim(label, key, content);
    const name = label ?? defaultLabel(position, content);
    if (plan.until !== undefined && name === plan.until) {
      closed = true;
      host.stop();
    }
    if (given.has(name)) {
      const result = given.get(name) ?? null;
      host.event({ type: "stepReplayed", position, label: name });
      host.record(
        { position, label: name, key, status: "succeeded", result, given: true, seq: nextSeq++ },
        0,
      );
      return JSON.parse(JSON.stringify(result)) as Json;
    }
    if (prior && finished(prior)) {
      return new Promise<Json>((resolve) => {
        replaying.push({
          // Steps recorded before finishing order was kept replay in the
          // order they were issued.
          order: prior.seq ?? prior.position,
          settle() {
            settleClock(prior);
            host.event({ type: "stepReplayed", position, label: name });
            resolve(prior.result ?? null);
          },
        });
        host.wake?.();
      });
    }

    if (host.canceled()) {
      closed = true;
      throw new CanceledError();
    }
    host.event({ type: "stepStarted", position, label: name });
    const started = host.now();
    // A wait counts from when it first began, also when it was interrupted
    // and goes on now, so that is recorded before it starts. One that failed
    // ran out of time, and starts over.
    const since = !WAITS.has(content.kind)
      ? undefined
      : prior && prior.status !== "failed" && typeof prior.since === "number"
        ? prior.since
        : started;
    if (since !== undefined) {
      host.record({ position, label: name, key, status: "waiting", since }, 0);
    }
    let attempt = 1;
    const record = (entry: Pick<StepRecord, "status"> & Partial<StepRecord>) =>
      host.record(
        {
          position,
          label: name,
          key,
          seq: nextSeq++,
          ...(since !== undefined && { since }),
          ...(attempt > 1 && { attempts: attempt }),
          ...entry,
        },
        host.now() - started,
      );
    const request: StartRequest = {
      position,
      label: name,
      task: content,
      fn,
      ...(policy.timeout !== undefined && { timeoutMs: policy.timeout }),
      ...(since !== undefined && { since }),
      ...(alongside && { alongside }),
    };
    let outcome: Outcome;
    for (;;) {
      let error: Error;
      try {
        outcome = await host.start(request);
        break;
      } catch (cause) {
        error = asError(cause);
      }
      // A failed step runs again while it has attempts left and the run is
      // still going.
      if (!interruptionOf(error) && attempt < policy.attempts && !closed) {
        host.event({
          type: "stepRetrying",
          position,
          label: name,
          attempt,
          attempts: policy.attempts,
          error: messageOf(error),
        });
        try {
          if (policy.retryDelay > 0) {
            await host.start({
              position,
              label: name,
              task: { kind: "sleep", duration: policy.retryDelay },
            });
          }
          attempt++;
          continue;
        } catch (cause) {
          // Canceled between attempts.
          error = asError(cause);
        }
      }
      // A step that was canceled or waits for an approval runs again when
      // the run is resumed; nothing else starts in this execution.
      const interrupted = interruptionOf(error);
      if (interrupted) {
        closed = true;
        record({ status: interrupted, error: messageOf(error) });
        throw error;
      }
      record({ status: "failed", error: messageOf(error) });
      throw new StepError(name, error);
    }
    const result = outcome.result ?? null;
    const finishedAt = host.now();
    // Some files failed: the step finished, and the rest of the run goes on.
    if (outcome.partial !== undefined) {
      record({ status: "partial", result, finishedAt, error: outcome.partial });
    } else {
      record({ status: "succeeded", result, finishedAt });
    }
    settleClock({ finishedAt });
    return result;
  }

  /** Whether a recorded step finished, so a resume reuses its result. */
  function finished(entry: StepRecord): boolean {
    return entry.status === "succeeded" || entry.status === "partial";
  }

  function interruptionOf(error: Error): "canceled" | "awaiting-approval" | null {
    if (error instanceof CanceledError) return "canceled";
    if (error instanceof AwaitingApprovalError) return "awaiting-approval";
    return null;
  }

  /** Captured values of an extracted callback; none for callbacks run in place. */
  function capturesOf(callback: Extracted): Json | undefined {
    if (typeof callback.captures !== "function") return undefined;
    const captures = callback.captures();
    checkJson(captures, "captures");
    return captures;
  }

  /** Turn a host rejection (`{ name, message }`) back into the runtime's errors. */
  function asError(cause: unknown): Error {
    if (cause instanceof Error) {
      if (cause.name === "CanceledError" && !(cause instanceof CanceledError)) {
        return new CanceledError(cause.message);
      }
      if (cause.name === "AwaitingApprovalError" && !(cause instanceof AwaitingApprovalError)) {
        return new AwaitingApprovalError(cause.message);
      }
      return cause;
    }
    if (isRecord(cause) && typeof cause.message === "string") {
      if (cause.name === "CanceledError") return new CanceledError(cause.message);
      if (cause.name === "AwaitingApprovalError") return new AwaitingApprovalError(cause.message);
      return new Error(cause.message);
    }
    return new Error(String(cause));
  }

  const { callbackOf: _, ...builders } = tasks;
  return {
    ...builders,
    __evaluate,
    step,
    sleep,
    waitForEvent,
    replayNext,
    close,
    isClosed,
  };
}

function isTask(value: unknown): value is Task {
  return isRecord(value) && TASK in value;
}

function isHandler(value: unknown): value is AstGrepHandler {
  return isRecord(value) && HANDLER in value;
}

/** `content` with `value` at a dotted `field` (`method.function`). */
function withField(content: TaskContent, field: string, value: Json): TaskContent {
  const [head, ...tail] = field.split(".");
  if (head === undefined) return content;
  if (tail.length === 0) return { ...content, [head]: value } as TaskContent;
  const inner = isRecord(content[head]) ? (content[head] as TaskContent) : ({} as TaskContent);
  return { ...content, [head]: withField(inner, tail.join("."), value) } as TaskContent;
}

/**
 * `value` without its `undefined` fields. A cycle is left in place (the
 * nested value as given), so `checkJson` reports it by its path.
 */
function withoutUndefined(value: unknown, seen: Set<object> = new Set()): unknown {
  if (typeof value !== "object" || value === null || seen.has(value)) return value;
  const plain = Array.isArray(value) || Object.getPrototypeOf(value) === Object.prototype;
  if (!plain) return value;
  seen.add(value);
  let result: unknown;
  if (Array.isArray(value)) {
    result = value.map((item) => withoutUndefined(item, seen));
  } else {
    const cleaned: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value)) {
      if (item !== undefined) cleaned[key] = withoutUndefined(item, seen);
    }
    result = cleaned;
  }
  seen.delete(value);
  return result;
}

function normalizeSelector(selector: unknown): Selector {
  if (selector === undefined) selector = {};
  if (typeof selector === "string" || Array.isArray(selector)) selector = { include: selector };
  if (!isRecord(selector)) {
    throw new TypeError(
      "forEachFile: the selector must be a glob, an array of globs, or an object",
    );
  }
  const known = new Set([
    "include",
    "exclude",
    "language",
    "containing",
    "concurrency",
    "semanticAnalysis",
    "capabilities",
  ]);
  for (const key of Object.keys(selector)) {
    if (!known.has(key)) throw new TypeError(`forEachFile: unknown selector field '${key}'`);
  }
  const normalized: Selector = {
    include: globs(selector.include, "include"),
    exclude: globs(selector.exclude, "exclude"),
  };
  for (const field of ["language", "containing"] as const) {
    const value = selector[field];
    if (value === undefined) continue;
    if (typeof value !== "string") {
      throw new TypeError(`forEachFile: selector.${field} must be a string`);
    }
    normalized[field] = value;
  }
  const { concurrency } = selector;
  if (concurrency !== undefined) {
    if (typeof concurrency !== "number" || !Number.isInteger(concurrency) || concurrency < 1) {
      throw new TypeError("forEachFile: concurrency must be a whole number of at least 1");
    }
    normalized.concurrency = concurrency;
  }
  const { semanticAnalysis, capabilities } = selector;
  if (semanticAnalysis !== undefined) {
    const mode = isRecord(semanticAnalysis) ? semanticAnalysis.mode : semanticAnalysis;
    if (!["none", "file", "workspace"].includes(mode as string)) {
      throw new TypeError(
        'forEachFile: semanticAnalysis must be "none", "file", "workspace" or { mode, root }',
      );
    }
    checkJson(semanticAnalysis, "forEachFile: semanticAnalysis");
    normalized.semanticAnalysis = semanticAnalysis;
  }
  if (capabilities !== undefined) {
    const known = ["fs", "fetch", "child_process"];
    if (!Array.isArray(capabilities) || !capabilities.every((c) => known.includes(c))) {
      throw new TypeError(
        'forEachFile: capabilities must be an array of "fs", "fetch" and "child_process"',
      );
    }
    normalized.capabilities = [...capabilities];
  }
  // Neither include nor language: the package's languages from codemod.yaml.
  return normalized;
}

function globs(value: unknown, field: string, owner = "forEachFile: selector."): string[] {
  if (value === undefined) return [];
  if (typeof value === "string") return [value];
  if (Array.isArray(value) && value.every((item) => typeof item === "string")) return [...value];
  throw new TypeError(`${owner}${field} must be a glob or an array of globs`);
}

function defaultLabel(position: number, content: TaskContent): string {
  switch (content.kind) {
    case "forEachFile": {
      const { include, language } = content.selector as Selector;
      return `forEachFile ${include.length ? include.join(", ") : (language ?? "")}`.trimEnd();
    }
    case "function":
      return `step #${position}`;
    case "shell":
      return `shell ${truncate(content.command)}`;
    case "astGrep": {
      const ids = (content.rules as { id?: unknown }[]).map((rule) => rule.id).filter(Boolean);
      return ids.length ? `astGrep ${ids.join(", ")}` : "astGrep";
    }
    case "approval":
      return `approval: ${truncate(content.message)}`;
    case "sleep":
      return `sleep ${describeDuration(Number(content.duration))}`;
    case "waitForEvent":
      return `wait for ${String(content.name)}`;
    case "codemod":
      return `codemod ${String(content.source)}`;
    case "dependencyGraph": {
      const lockfiles = content.lockfiles as string[] | undefined;
      return lockfiles ? `dependencyGraph ${lockfiles.join(", ")}` : "dependencyGraph";
    }
    case "agent":
      return `agent: ${truncate(content.prompt)}`;
    default:
      return content.kind;
  }
}

function truncate(text: unknown, length = 60): string {
  const line = String(text ?? "").split("\n")[0] ?? "";
  return line.length > length ? `${line.slice(0, length - 1)}…` : line;
}

function canonical(value: unknown): string {
  return JSON.stringify(sortKeys(value));
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (isRecord(value)) {
    const sorted: Record<string, unknown> = {};
    for (const key of Object.keys(value).sort()) sorted[key] = sortKeys(value[key]);
    return sorted;
  }
  return value;
}

/** Freeze a JSON value, all the way down. */
export function freeze<T>(value: T): T {
  if (isRecord(value) && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const key of Object.keys(value)) freeze(value[key]);
  }
  return value;
}

/**
 * An error as a step's failure says it. Changing a captured value or a step
 * result fails as a TypeError that does not say why; the hint does.
 */
export function describeError(error: unknown): string {
  if (!isRecord(error)) return String(error);
  let message = `${String(error.name || "Error")}: ${String(error.message)}`;
  if (
    error instanceof TypeError &&
    /extensible|read-only|read only|frozen|non-writable/i.test(error.message)
  ) {
    message += " (captured values and step results are frozen; copy one before changing it)";
  }
  return message;
}

/** What a `forEachFile` callback did to its file. */
export type FileEffect =
  | { op: "write"; text: string }
  | { op: "rename"; to: string }
  | { op: "delete" };

/** The `file` a `forEachFile` callback gets. */
export interface WorkflowFile {
  readonly path: string;
  readonly text: string;
  readonly language: string | undefined;
  write(text: string): void;
  rename(to: string): void;
  delete(): void;
}

/**
 * The `file` a `forEachFile` callback gets, and the effects it records on it.
 * After `close()`, when the callback has finished, the file takes no more.
 */
export function fileHandle(
  path: string,
  text: string,
  language?: string,
): { file: WorkflowFile; effects: FileEffect[]; close(): void } {
  const effects: FileEffect[] = [];
  let open = true;
  const guard = (name: string) => {
    if (!open) throw new Error(`file.${name}() was called after the callback finished`);
  };
  const file: WorkflowFile = Object.freeze({
    path,
    text,
    language,
    write(next: string) {
      guard("write");
      if (typeof next !== "string") throw new TypeError("file.write(text) expects a string");
      effects.push({ op: "write", text: next });
    },
    rename(to: string) {
      guard("rename");
      if (typeof to !== "string" || to === "") {
        throw new TypeError("file.rename(path) expects a non-empty path");
      }
      effects.push({ op: "rename", to });
    },
    delete() {
      guard("delete");
      effects.push({ op: "delete" });
    },
  });
  return {
    file,
    effects,
    close() {
      open = false;
    },
  };
}

function messageOf(error: unknown): string {
  return isRecord(error) && "message" in error ? String(error.message) : String(error);
}

/** Throw a TypeError naming the first part of `value` that is not JSON. */
export function checkJson(
  value: unknown,
  path: string,
  seen: Set<object> = new Set(),
): asserts value is Json {
  if (value === null || typeof value === "string" || typeof value === "boolean") return;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new TypeError(`${path} is ${value}, which is not JSON`);
    return;
  }
  if (typeof value !== "object") {
    const what = value === undefined ? "undefined" : `a ${typeof value}`;
    throw new TypeError(`${path} is ${what}, which is not JSON`);
  }
  if (seen.has(value)) throw new TypeError(`${path} is a circular reference`);
  seen.add(value);
  if (Array.isArray(value)) {
    value.forEach((item, index) => checkJson(item, `${path}[${index}]`, seen));
  } else {
    const proto: unknown = Object.getPrototypeOf(value);
    if (proto !== Object.prototype && proto !== null) {
      const name = value.constructor?.name || "non-plain object";
      throw new TypeError(`${path} is a ${name}, which is not JSON`);
    }
    for (const [key, item] of Object.entries(value)) checkJson(item, `${path}.${key}`, seen);
  }
  seen.delete(value);
}

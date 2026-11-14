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

/** A JSON value: what steps record, tasks are described in, and state holds. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

// ------------------------------------------------------- the run's records ---

export type StepStatus = "succeeded" | "failed" | "canceled" | "awaiting-approval" | "waiting";

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
  /** State the step set: keys to values, where `null` removes a key. */
  state?: Record<string, Json> | null;
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
  /** Workflow state when the step was called. */
  state: Record<string, Json>;
  /** The step's function, on hosts that run callbacks in place. */
  fn?: Callback;
  /** How long the task may run, from the step's `timeout`. */
  timeoutMs?: number;
  /** When the step began to wait (`sleep`, `waitForEvent`). */
  since?: number;
}

/** What a task resolves to: `TaskOutcome` in `crates/workflow/src/tasks/mod.rs`. */
export interface Outcome {
  result?: Json;
  /** State the task set: keys to values, where `null` removes a key. */
  state?: Record<string, Json> | null;
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
}

export interface ForEachFileTask {
  readonly [TASK]: "forEachFile";
  readonly selector: Selector;
  readonly callback: Extracted;
}

export interface HostTask {
  readonly [TASK]: "host";
  readonly content: TaskContent;
}

export type Task = ForEachFileTask | HostTask;

/** A length of time: milliseconds, or a string such as "30s" or "10m". */
export type Duration = number | string;

/** `step(..., options)`: how to run a step. */
export interface StepOptions {
  retries?: number;
  retryDelay?: Duration;
  timeout?: Duration;
}

/**
 * The task builders (`forEachFile`, `shell`, `jssg`, ...). They only describe
 * work, so they need no run. `options.callbackId(fn)` lets callbacks be given
 * as functions, for hosts that run them in place.
 */
export function createTasks(options: Pick<Host, "callbackId"> = {}) {
  /** Inserted by the build in place of an extracted callback. */
  function __callback(id: string, captures: () => Json): CallbackReference {
    return { __flowCallback: id, captures };
  }

  /** A callback the build extracted, or one given as a function on hosts that run them in place. */
  function callbackOf(value: unknown): Extracted | null {
    if (isRecord(value) && typeof value.__flowCallback === "string") {
      return { id: value.__flowCallback, captures: value.captures as Extracted["captures"] };
    }
    if (typeof value === "function" && typeof options.callbackId === "function") {
      const fn = value as Callback;
      return { id: options.callbackId(fn), fn };
    }
    return null;
  }

  function forEachFile(selector: unknown, callback?: unknown): ForEachFileTask {
    if (callback === undefined && callbackOf(selector)) {
      callback = selector;
      selector = undefined;
    }
    const extracted = callbackOf(callback);
    if (!extracted) {
      throw new TypeError(
        "forEachFile: the callback was not extracted by the build; run the workflow through the codemod-workflow build",
      );
    }
    return Object.freeze({
      [TASK]: "forEachFile" as const,
      selector: normalizeSelector(selector),
      callback: extracted,
    });
  }

  function hostTask(kind: string, options: unknown): HostTask {
    const content = withoutUndefined(options);
    checkJson(content, kind);
    return Object.freeze({
      [TASK]: "host" as const,
      content: { ...(content as Record<string, unknown>), kind },
    });
  }

  /** Run a shell command. */
  function shell(command: string, options: object = {}): HostTask {
    return hostTask("shell", { ...options, command });
  }

  /** Run a JSSG transform file over the target. */
  function jssg(path: string, options: object = {}): HostTask {
    return hostTask("jssg", { ...options, path });
  }

  /** Apply an ast-grep YAML rule file. */
  function astGrep(path: string, options: object = {}): HostTask {
    return hostTask("astGrep", { ...options, path });
  }

  /** Split files into shards. */
  function shard(options: object): HostTask {
    return hostTask("shard", options);
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

  /** Check out a branch for the following changes (managed git only). */
  function branch(name: string): HostTask {
    return hostTask("branch", { name });
  }

  /** Commit changes (managed git only). */
  function commit(message: string, options: object = {}): HostTask {
    return hostTask("commit", { ...options, message });
  }

  /** Push the branch and open a pull request (managed git only). */
  function pullRequest(options: object): HostTask {
    return hostTask("pullRequest", options);
  }

  return {
    callbackOf,
    __callback,
    forEachFile,
    shell,
    jssg,
    astGrep,
    shard,
    codemod,
    agent,
    installSkill,
    approval,
    sleep,
    waitForEvent,
    branch,
    commit,
    pullRequest,
  };
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
    if (entry.status !== "succeeded") unfinished.add(entry);
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

  const state = new Map<string, Json>();

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
  // state, the clock and promise races as it did then.
  const replaying: Replay[] = [];

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

  // ---------------------------------------------------------------- state ---

  /** Read one state key, or the whole state as an object when called without one. Values are frozen. */
  function getState(key?: string): Json | undefined | Readonly<Record<string, Json>> {
    if (key === undefined) return Object.freeze(Object.fromEntries(state));
    return state.get(key);
  }

  /** Set workflow state. `undefined` removes the key. */
  function setState(key: string, value: unknown): void {
    if (value === undefined) {
      state.delete(key);
      return;
    }
    checkJson(value, `state.${key}`);
    state.set(key, freeze(JSON.parse(JSON.stringify(value)) as Json));
  }

  function unsetState(key: string): void {
    state.delete(key);
  }

  function applyState(changes: Outcome["state"]): void {
    if (!changes) return;
    for (const [key, value] of Object.entries(changes)) {
      if (value === null) state.delete(key);
      else state.set(key, freeze(value));
    }
  }

  function stateSnapshot(): Record<string, Json> {
    return Object.fromEntries(state);
  }

  // The body runs alone, so a lock there has nothing to wait for; steps and
  // callbacks get the real one from their own runtime.
  function acquireLock(_name: string): () => void {
    return () => {};
  }

  // Step outputs are kept for the transforms that still set them; nothing in
  // a workflow reads them back but these functions.
  const stepOutputs = new Map<string, string>();
  const outputKey = (stepId: string, name: string) => JSON.stringify([stepId, name]);

  function setStepOutput(name: string, value: unknown): void {
    stepOutputs.set(outputKey("", name), String(value));
  }

  function getStepOutput(stepId: string, name: string): string | null {
    return stepOutputs.get(outputKey(stepId, name)) ?? null;
  }

  function getOrSetStepOutput(stepId: string, name: string, value: unknown): string {
    const key = outputKey(stepId, name);
    const existing = stepOutputs.get(key);
    if (existing !== undefined) return existing;
    const text = String(value);
    stepOutputs.set(key, text);
    return text;
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
    return runStep(label, args[0], args[1]).then(freeze);
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
  ): Promise<Json> {
    const described = label === undefined ? "step" : `step '${label}'`;
    const policy = policyOf(options, described);
    // Everything up to the first await runs synchronously inside step(), so
    // captures and state are a snapshot taken when the step is called.
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
    } else if (isTask(task) && task[TASK] === "host") {
      content = task.content;
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
    if (prior && prior.status === "succeeded") {
      return new Promise<Json>((resolve) => {
        replaying.push({
          // Steps recorded before finishing order was kept replay in the
          // order they were issued.
          order: prior.seq ?? prior.position,
          settle() {
            applyState(prior.state);
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
      state: stateSnapshot(),
      fn,
      ...(policy.timeout !== undefined && { timeoutMs: policy.timeout }),
      ...(since !== undefined && { since }),
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
              state: {},
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
    const changes = outcome.state ?? null;
    applyState(changes);
    const finishedAt = host.now();
    record({ status: "succeeded", result, state: changes, finishedAt });
    settleClock({ finishedAt });
    return result;
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
    getState,
    setState,
    unsetState,
    acquireLock,
    setStepOutput,
    getStepOutput,
    getOrSetStepOutput,
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

function withoutUndefined(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(withoutUndefined);
  if (isRecord(value) && Object.getPrototypeOf(value) === Object.prototype) {
    const result: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value)) {
      if (item !== undefined) result[key] = withoutUndefined(item);
    }
    return result;
  }
  return value;
}

function normalizeSelector(selector: unknown): Selector {
  if (selector === undefined) selector = {};
  if (typeof selector === "string" || Array.isArray(selector)) selector = { include: selector };
  if (!isRecord(selector)) {
    throw new TypeError(
      "forEachFile: the selector must be a glob, an array of globs, or an object",
    );
  }
  const known = new Set(["include", "exclude", "language", "containing", "concurrency"]);
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
  // Neither include nor language: the package's languages from codemod.yaml.
  return normalized;
}

function globs(value: unknown, field: string): string[] {
  if (value === undefined) return [];
  if (typeof value === "string") return [value];
  if (Array.isArray(value) && value.every((item) => typeof item === "string")) return [...value];
  throw new TypeError(`forEachFile: selector.${field} must be a glob or an array of globs`);
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
    case "jssg":
    case "astGrep":
      return `${content.kind} ${String(content.path)}`;
    case "approval":
      return `approval: ${truncate(content.message)}`;
    case "sleep":
      return `sleep ${describeDuration(Number(content.duration))}`;
    case "waitForEvent":
      return `wait for ${String(content.name)}`;
    case "codemod":
      return `codemod ${String(content.source)}`;
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

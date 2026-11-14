// Generated from crates/workflow/src/js/orchestrator.js by `pnpm sync-orchestrator`; do not edit.

// The workflow orchestrator: step(), replay, test plans and the task
// builders that make up `codemod/workflow`. It holds no host globals, so the
// same code runs in the engine's QuickJS context and in Node.js or Bun.
//
// `createRuntime(host)` returns the module's exports for one run. The host:
//   history(): StepRecord[]               steps recorded by earlier runs
//   record(entry, ms)                     persist one step's outcome
//   event(event)                          report step started / replayed / retrying
//   canceled(): boolean                   whether a step about to start must not,
//                                         because the run was asked to stop
//   start(request): Promise<outcome>      run a task; rejects with { name, message }.
//                                         `timeoutMs` bounds it; `since` is when a
//                                         waiting step began to wait
//   evaluate(kind, expression, scope)     evaluate a workflow.yaml `${{ }}` expression
//   now(): number                         the real time, for step records
//   advanceClock(ms)                      move the body's clock forward
//   testPlan(): { until?, given? } | null when the run is a test
//   stop()                                the test's `until` step has started
//   wake?()                               a replayed step waits for `replayNext()`
//   callbackId?(fn): string               identifies a callback given as a function;
//                                         hosts without it need the build to extract callbacks
//
// The host drives two more things, on the object `createRuntime` returns:
//   replayNext(): boolean   settle the next replayed step; call it whenever the
//                           workflow has nothing left to run, until it is false
//   close()                 the run is ending: no step starts from here on
//   isClosed(): boolean     whether steps issued now would never start

export class StepError extends Error {
  constructor(label, cause) {
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
  constructor(message) {
    super(message);
    this.name = "AwaitingApprovalError";
  }
}

const TASK = Symbol.for("codemod.workflow.task");

/**
 * The task builders (`forEachFile`, `shell`, `jssg`, ...). They only describe
 * work, so they need no run. `options.callbackId(fn)` lets callbacks be given
 * as functions, for hosts that run them in place.
 */
export function createTasks(options = {}) {
  /** Inserted by the build in place of an extracted callback. */
  function __callback(id, captures) {
    return { __flowCallback: id, captures };
  }

  /** A callback the build extracted, or one given as a function on hosts that run them in place. */
  function callbackOf(value) {
    if (value && typeof value.__flowCallback === "string") {
      return { id: value.__flowCallback, captures: value.captures };
    }
    if (typeof value === "function" && typeof options.callbackId === "function") {
      return { id: options.callbackId(value), fn: value };
    }
    return null;
  }

  function forEachFile(selector, callback) {
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
      [TASK]: "forEachFile",
      selector: normalizeSelector(selector),
      callback: extracted,
    });
  }

  function hostTask(kind, options) {
    const content = withoutUndefined(options);
    checkJson(content, kind);
    return Object.freeze({ [TASK]: "host", content: { ...content, kind } });
  }

  /** Run a shell command. */
  function shell(command, options = {}) {
    return hostTask("shell", { ...options, command });
  }

  /** Run a JSSG transform file over the target. */
  function jssg(path, options = {}) {
    return hostTask("jssg", { ...options, path });
  }

  /** Apply an ast-grep YAML rule file. */
  function astGrep(path, options = {}) {
    return hostTask("astGrep", { ...options, path });
  }

  /** Split files into shards. */
  function shard(options) {
    return hostTask("shard", options);
  }

  /** Run another codemod package (registry name or local path). */
  function codemod(source, options = {}) {
    return hostTask("codemod", { ...options, source });
  }

  /** Hand a prompt to a coding agent. */
  function agent(prompt, options = {}) {
    return hostTask("agent", { ...options, prompt });
  }

  /** Install an agent skill package. */
  function installSkill(options) {
    return hostTask("installSkill", options);
  }

  /** Wait for a person to approve before continuing. */
  function approval(message) {
    return hostTask("approval", { message });
  }

  /** Wait for a time: milliseconds, or a string such as "30s" or "10m". */
  function sleep(duration) {
    return hostTask("sleep", { duration: milliseconds(duration, "sleep") });
  }

  /** Wait for an event sent to the run (`codemod workflow send`). */
  function waitForEvent(name, options = {}) {
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
  function branch(name) {
    return hostTask("branch", { name });
  }

  /** Commit changes (managed git only). */
  function commit(message, options = {}) {
    return hostTask("commit", { ...options, message });
  }

  /** Push the branch and open a pull request (managed git only). */
  function pullRequest(options) {
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

function onlyKeys(options, known, what) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError(`${what}: options must be an object`);
  }
  for (const key of Object.keys(options)) {
    if (!known.includes(key)) {
      throw new TypeError(`${what}: unknown option '${key}'; expected ${known.join(", ")}`);
    }
  }
}

const UNITS = { ms: 1, s: 1000, m: 60_000, h: 3_600_000, d: 86_400_000 };

/** A duration in milliseconds, from a number of them or a string such as "30s". */
export function milliseconds(value, what) {
  if (typeof value === "number" && Number.isFinite(value) && value >= 0) return Math.round(value);
  const match = typeof value === "string" && /^(\d+(?:\.\d+)?)\s*(ms|s|m|h|d)$/.exec(value.trim());
  if (!match) {
    throw new TypeError(
      `${what}: expected milliseconds or a duration such as "500ms", "30s", "10m", "1h" or "2d"`,
    );
  }
  return Math.round(Number(match[1]) * UNITS[match[2]]);
}

/** A duration in its largest whole unit, as the engine words it. */
export function describeDuration(ms) {
  for (const unit of ["h", "m", "s"]) {
    if (ms >= UNITS[unit] && ms % UNITS[unit] === 0) return `${ms / UNITS[unit]}${unit}`;
  }
  return `${ms}ms`;
}

/** What `step(..., options)` asks for: how often to try, and for how long. */
function policyOf(options, what) {
  if (options === undefined) return { attempts: 1, retryDelay: 0, timeout: undefined };
  onlyKeys(options, ["retries", "retryDelay", "timeout"], what);
  const { retries = 0, retryDelay = "1s", timeout } = options;
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

export function createRuntime(host) {
  const tasks = createTasks(host);
  const { callbackOf } = tasks;

  // What earlier executions of the run recorded. A step is the one recorded
  // when it has the same label and content (its key), wherever and whenever
  // the body issues it: the order steps are issued in depends on how fast
  // other steps finish, so it cannot say which step is which. Steps that are
  // the same in both are interchangeable and take the records in turn.
  const recorded = new Map();
  // Recorded steps that did not finish, for a changed step to take over.
  const unfinished = new Set();
  // A step keeps its position for the whole run; new steps go after.
  let nextPosition = 0;
  // Steps are numbered in the order they finish, across executions of the run.
  let nextSeq = 0;
  for (const entry of [...host.history()].sort((a, b) => a.position - b.position)) {
    if (!recorded.has(entry.key)) recorded.set(entry.key, []);
    recorded.get(entry.key).push(entry);
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
  function claim(label, key, content) {
    const same = recorded.get(key);
    if (same?.length) {
      const entry = same.shift();
      unfinished.delete(entry);
      return { position: entry.position, entry };
    }
    for (const entry of unfinished) {
      if (entry.label !== (label ?? defaultLabel(entry.position, content))) continue;
      unfinished.delete(entry);
      const others = recorded.get(entry.key);
      others.splice(others.indexOf(entry), 1);
      return { position: entry.position };
    }
    return { position: nextPosition++ };
  }

  const state = new Map();

  // A test run can stop after one step (`until`) and skip steps with a given
  // result (`given`), both matched by label.
  const plan = host.testPlan() ?? {};
  const given = new Map(Object.entries(plan.given ?? {}));

  // Once closed, no step starts: steps issued from then on never settle. A
  // run closes when a test's `until` step starts, when a step is canceled or
  // waits for an approval, and when the host ends the run.
  let closed = false;

  // Recorded steps the body has issued again, waiting to settle. They settle
  // one at a time in the order they finished when they ran, so the body sees
  // state, the clock and promise races as it did then.
  const replaying = [];

  function replayNext() {
    if (closed || replaying.length === 0) return false;
    let next = 0;
    for (let i = 1; i < replaying.length; i++) {
      if (replaying[i].order < replaying[next].order) next = i;
    }
    replaying.splice(next, 1)[0].settle();
    return true;
  }

  function close() {
    closed = true;
  }

  function isClosed() {
    return closed;
  }

  // The body's clock only moves when a step finishes, to the time recorded
  // for it, so Date.now() in the body gives the same answer on every replay.
  function settleClock(entry) {
    if (entry && typeof entry.finishedAt === "number") host.advanceClock(entry.finishedAt);
  }

  // ---------------------------------------------------------------- state ---

  /** Read one state key, or the whole state as an object when called without one. Values are frozen. */
  function getState(key) {
    if (key === undefined) return Object.freeze(Object.fromEntries(state));
    return state.get(key);
  }

  /** Set workflow state. `undefined` removes the key. */
  function setState(key, value) {
    if (value === undefined) {
      state.delete(key);
      return;
    }
    checkJson(value, `state.${key}`);
    state.set(key, freeze(JSON.parse(JSON.stringify(value))));
  }

  function unsetState(key) {
    state.delete(key);
  }

  function applyState(changes) {
    if (!changes) return;
    for (const [key, value] of Object.entries(changes)) {
      if (value === null) state.delete(key);
      else state.set(key, freeze(value));
    }
  }

  function stateSnapshot() {
    return Object.fromEntries(state);
  }

  // The body runs alone, so a lock there has nothing to wait for; steps and
  // callbacks get the real one from their own runtime.
  function acquireLock(_name) {
    return () => {};
  }

  // Step outputs are kept for the transforms that still set them; nothing in
  // a workflow reads them back but these functions.
  const stepOutputs = new Map();
  const outputKey = (stepId, name) => JSON.stringify([stepId, name]);

  function setStepOutput(name, value) {
    stepOutputs.set(outputKey("", name), String(value));
  }

  function getStepOutput(stepId, name) {
    return stepOutputs.get(outputKey(stepId, name)) ?? null;
  }

  function getOrSetStepOutput(stepId, name, value) {
    const key = outputKey(stepId, name);
    if (!stepOutputs.has(key)) stepOutputs.set(key, String(value));
    return stepOutputs.get(key);
  }

  /** Evaluate a workflow.yaml `${{ }}` expression; used by translated workflows. */
  function __evaluate(kind, expression, scope) {
    return host.evaluate(kind, expression, scope);
  }

  // ----------------------------------------------------------------- step ---

  /**
   * Run a task as a durable step. Starts immediately; the returned promise
   * resolves to the step's frozen result. `options` may give `retries`,
   * `retryDelay` and `timeout`; they say how to run the step, not what it
   * is, so changing them does not make it another step.
   */
  function step(...args) {
    const label = typeof args[0] === "string" ? args.shift() : undefined;
    return runStep(label, args[0], args[1]).then(freeze);
  }

  /** Wait for a time, as a step: a resumed run waits for what is left. */
  function sleep(duration) {
    return step(tasks.sleep(duration));
  }

  /** Wait for an event sent to the run, as a step; resolves to the event's data. */
  function waitForEvent(name, options) {
    return step(tasks.waitForEvent(name, options));
  }

  async function runStep(label, task, options) {
    const described = label === undefined ? "step" : `step '${label}'`;
    const policy = policyOf(options, described);
    // Everything up to the first await runs synchronously inside step(), so
    // captures and state are a snapshot taken when the step is called.
    let content;
    let fn;
    const callback = callbackOf(task);
    if (callback) {
      content = { kind: "function", callback: callback.id, captures: capturesOf(callback) };
      fn = callback.fn;
    } else if (typeof task === "function") {
      throw new TypeError(
        `${described}: write the function inline, as step(() => ...), so the build can run it in its own isolate`,
      );
    } else if (task && task[TASK] === "forEachFile") {
      content = {
        kind: "forEachFile",
        selector: task.selector,
        callback: task.callback.id,
        captures: capturesOf(task.callback),
      };
      fn = task.callback.fn;
    } else if (task && task[TASK] === "host") {
      content = task.content;
    } else {
      throw new TypeError(`${described}: expected a task such as forEachFile(...) or a function`);
    }

    const key = canonical([label ?? null, content]);
    if (closed) return new Promise(() => {});
    const { position, entry: prior } = claim(label, key, content);
    const name = label ?? defaultLabel(position, content);
    if (plan.until !== undefined && name === plan.until) {
      closed = true;
      host.stop();
    }
    if (given.has(name)) {
      const result = given.get(name);
      host.event({ type: "stepReplayed", position, label: name });
      host.record(
        { position, label: name, key, status: "succeeded", result, given: true, seq: nextSeq++ },
        0,
      );
      return JSON.parse(JSON.stringify(result ?? null));
    }
    if (prior && prior.status === "succeeded") {
      return new Promise((resolve) => {
        replaying.push({
          // Steps recorded before finishing order was kept replay in the
          // order they were issued.
          order: prior.seq ?? prior.position,
          settle() {
            applyState(prior.state);
            settleClock(prior);
            host.event({ type: "stepReplayed", position, label: name });
            resolve(prior.result);
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
    const record = (entry) =>
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
    const request = {
      position,
      label: name,
      task: content,
      state: stateSnapshot(),
      fn,
      ...(policy.timeout !== undefined && { timeoutMs: policy.timeout }),
      ...(since !== undefined && { since }),
    };
    let outcome;
    for (;;) {
      let error;
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

  function interruptionOf(error) {
    if (error instanceof CanceledError) return "canceled";
    if (error instanceof AwaitingApprovalError) return "awaiting-approval";
    return null;
  }

  /** Captured values of an extracted callback; none for callbacks run in place. */
  function capturesOf(callback) {
    if (typeof callback.captures !== "function") return undefined;
    const captures = callback.captures();
    checkJson(captures, "captures");
    return captures;
  }

  /** Turn a host rejection (`{ name, message }`) back into the runtime's errors. */
  function asError(cause) {
    if (cause instanceof Error) {
      if (cause.name === "CanceledError" && !(cause instanceof CanceledError)) {
        return new CanceledError(cause.message);
      }
      if (cause.name === "AwaitingApprovalError" && !(cause instanceof AwaitingApprovalError)) {
        return new AwaitingApprovalError(cause.message);
      }
      return cause;
    }
    if (cause && typeof cause === "object" && typeof cause.message === "string") {
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

function withoutUndefined(value) {
  if (Array.isArray(value)) return value.map(withoutUndefined);
  if (value && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) {
    const result = {};
    for (const [key, item] of Object.entries(value)) {
      if (item !== undefined) result[key] = withoutUndefined(item);
    }
    return result;
  }
  return value;
}

function normalizeSelector(selector) {
  if (selector === undefined) selector = {};
  if (typeof selector === "string" || Array.isArray(selector)) selector = { include: selector };
  if (!selector || typeof selector !== "object") {
    throw new TypeError(
      "forEachFile: the selector must be a glob, an array of globs, or an object",
    );
  }
  const known = new Set(["include", "exclude", "language", "containing", "concurrency"]);
  for (const key of Object.keys(selector)) {
    if (!known.has(key)) throw new TypeError(`forEachFile: unknown selector field '${key}'`);
  }
  const normalized = {
    include: globs(selector.include, "include"),
    exclude: globs(selector.exclude, "exclude"),
  };
  for (const field of ["language", "containing"]) {
    if (selector[field] === undefined) continue;
    if (typeof selector[field] !== "string") {
      throw new TypeError(`forEachFile: selector.${field} must be a string`);
    }
    normalized[field] = selector[field];
  }
  if (selector.concurrency !== undefined) {
    if (!Number.isInteger(selector.concurrency) || selector.concurrency < 1) {
      throw new TypeError("forEachFile: concurrency must be a whole number of at least 1");
    }
    normalized.concurrency = selector.concurrency;
  }
  // Neither include nor language: the package's languages from codemod.yaml.
  return normalized;
}

function globs(value, field) {
  if (value === undefined) return [];
  if (typeof value === "string") return [value];
  if (Array.isArray(value) && value.every((item) => typeof item === "string")) return [...value];
  throw new TypeError(`forEachFile: selector.${field} must be a glob or an array of globs`);
}

function defaultLabel(position, content) {
  switch (content.kind) {
    case "forEachFile": {
      const { include, language } = content.selector;
      return `forEachFile ${include.length ? include.join(", ") : (language ?? "")}`.trimEnd();
    }
    case "function":
      return `step #${position}`;
    case "shell":
      return `shell ${truncate(content.command)}`;
    case "jssg":
    case "astGrep":
      return `${content.kind} ${content.path}`;
    case "approval":
      return `approval: ${truncate(content.message)}`;
    case "sleep":
      return `sleep ${describeDuration(content.duration)}`;
    case "waitForEvent":
      return `wait for ${content.name}`;
    case "codemod":
      return `codemod ${content.source}`;
    case "agent":
      return `agent: ${truncate(content.prompt)}`;
    default:
      return content.kind;
  }
}

function truncate(text, length = 60) {
  const line = String(text ?? "").split("\n")[0];
  return line.length > length ? `${line.slice(0, length - 1)}…` : line;
}

function canonical(value) {
  return JSON.stringify(sortKeys(value));
}

function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === "object") {
    const sorted = {};
    for (const key of Object.keys(value).sort()) sorted[key] = sortKeys(value[key]);
    return sorted;
  }
  return value;
}

/** Freeze a JSON value, all the way down. */
export function freeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const key of Object.keys(value)) freeze(value[key]);
  }
  return value;
}

/**
 * An error as a step's failure says it. Changing a captured value or a step
 * result fails as a TypeError that does not say why; the hint does.
 */
export function describeError(error) {
  if (!error || typeof error !== "object") return String(error);
  let message = `${error.name || "Error"}: ${error.message}`;
  if (
    error instanceof TypeError &&
    /extensible|read-only|read only|frozen|non-writable/i.test(error.message)
  ) {
    message += " (captured values and step results are frozen; copy one before changing it)";
  }
  return message;
}

/**
 * The `file` a `forEachFile` callback gets, and the effects it records on it:
 * `{ op: "write", text }`, `{ op: "rename", to }` and `{ op: "delete" }`.
 * After `close()`, when the callback has finished, the file takes no more.
 */
export function fileHandle(path, text, language) {
  const effects = [];
  let open = true;
  const guard = (name) => {
    if (!open) throw new Error(`file.${name}() was called after the callback finished`);
  };
  const file = Object.freeze({
    path,
    text,
    language,
    write(next) {
      guard("write");
      if (typeof next !== "string") throw new TypeError("file.write(text) expects a string");
      effects.push({ op: "write", text: next });
    },
    rename(to) {
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

function messageOf(error) {
  return error && typeof error === "object" && "message" in error ? error.message : String(error);
}

/** Throw a TypeError naming the first part of `value` that is not JSON. */
export function checkJson(value, path, seen = new Set()) {
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
    const proto = Object.getPrototypeOf(value);
    if (proto !== Object.prototype && proto !== null) {
      const name = (value.constructor && value.constructor.name) || "non-plain object";
      throw new TypeError(`${path} is a ${name}, which is not JSON`);
    }
    for (const key of Object.keys(value)) checkJson(value[key], `${path}.${key}`, seen);
  }
  seen.delete(value);
}

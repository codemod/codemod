// Generated from crates/workflow/src/js/orchestrator.js by `pnpm sync-orchestrator`; do not edit.

// The workflow orchestrator: step(), replay, test plans and the task
// builders that make up `codemod:workflow`. It holds no host globals, so the
// same code runs in the engine's QuickJS context and in Node.js or Bun.
//
// `createRuntime(host)` returns the module's exports for one run. The host:
//   history(): StepRecord[]               steps recorded by earlier runs
//   record(entry, ms)                     persist one step's outcome
//   event(event)                          report step started / replayed
//   canceled(): boolean                   whether the run was asked to stop
//   start(request): Promise<outcome>      run a task; rejects with { name, message }
//   evaluate(kind, expression, scope)     evaluate a workflow.yaml `${{ }}` expression
//   now(): number                         the real time, for step records
//   advanceClock(ms)                      move the body's clock forward
//   testPlan(): { until?, given? } | null when the run is a test
//   stop()                                the test's `until` step has started
//   callbackId?(fn): string               identifies a callback given as a function;
//                                         hosts without it need the build to extract callbacks

export class NondeterminismError extends Error {
  constructor(message) {
    super(message);
    this.name = "NondeterminismError";
  }
}

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
    branch,
    commit,
    pullRequest,
  };
}

export function createRuntime(host) {
  const tasks = createTasks(host);
  const { callbackOf } = tasks;

  const recorded = new Map();
  for (const entry of host.history()) recorded.set(entry.position, entry);

  const state = new Map();
  let nextPosition = 0;

  // A test run can stop after one step (`until`) and skip steps with a given
  // result (`given`), both matched by label.
  const plan = host.testPlan() ?? {};
  const given = new Map(Object.entries(plan.given ?? {}));
  let stopped = false;

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

  /** Evaluate a workflow.yaml `${{ }}` expression; used by translated workflows. */
  function __evaluate(kind, expression, scope) {
    return host.evaluate(kind, expression, scope);
  }

  // ----------------------------------------------------------------- step ---

  /**
   * Run a task as a durable step. Starts immediately; the returned promise
   * resolves to the step's frozen result.
   */
  function step(labelOrTask, maybeTask) {
    const position = nextPosition++;
    const [label, task] =
      typeof labelOrTask === "string" ? [labelOrTask, maybeTask] : [undefined, labelOrTask];
    return runStep(position, label, task).then(freeze);
  }

  async function runStep(position, label, task) {
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
        `step #${position}: write the function inline, as step(() => ...), so the build can run it in its own isolate`,
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
      throw new TypeError(
        `step #${position}: expected a task such as forEachFile(...) or a function`,
      );
    }

    const name = label ?? defaultLabel(position, content);
    const key = canonical(content);
    // Steps started after a test's `until` step never run, and never settle.
    if (stopped) return new Promise(() => {});
    if (plan.until !== undefined && name === plan.until) {
      stopped = true;
      host.stop();
    }
    if (given.has(name)) {
      const result = given.get(name);
      host.event({ type: "stepReplayed", position, label: name });
      host.record({ position, label: name, key, status: "succeeded", result, given: true }, 0);
      return JSON.parse(JSON.stringify(result ?? null));
    }
    const prior = recorded.get(position);
    if (prior && prior.status === "succeeded") {
      if (prior.key !== key) {
        throw new NondeterminismError(
          `step #${position} ('${name}') differs from the recorded run; the workflow changed before this step, so its recorded result cannot be reused`,
        );
      }
      applyState(prior.state);
      settleClock(prior);
      host.event({ type: "stepReplayed", position, label: name });
      return prior.result;
    }

    if (host.canceled()) throw new CanceledError();
    host.event({ type: "stepStarted", position, label: name });
    const started = host.now();
    let result;
    let changes = null;
    try {
      const outcome = await host.start({
        position,
        label: name,
        task: content,
        state: stateSnapshot(),
        fn,
      });
      result = outcome.result ?? null;
      changes = outcome.state ?? null;
    } catch (cause) {
      const error = asError(cause);
      if (error instanceof CanceledError || error instanceof AwaitingApprovalError) throw error;
      host.record(
        { position, label: name, key, status: "failed", error: messageOf(error) },
        host.now() - started,
      );
      throw new StepError(name, error);
    }
    applyState(changes);
    const finishedAt = host.now();
    host.record(
      { position, label: name, key, status: "succeeded", result, state: changes, finishedAt },
      finishedAt - started,
    );
    settleClock({ finishedAt });
    return result;
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
  return { ...builders, getState, setState, unsetState, __evaluate, step };
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

function freeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const key of Object.keys(value)) freeze(value[key]);
  }
  return value;
}

function messageOf(error) {
  return error && typeof error === "object" && "message" in error ? error.message : String(error);
}

function checkJson(value, path, seen = new Set()) {
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

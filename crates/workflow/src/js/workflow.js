// The `codemod:workflow` module, as seen by the workflow's own context.
//
// The host installs `globalThis.__workflowHost` before this module evaluates:
//   history(): string                    steps recorded by earlier runs, as JSON
//   record(entry: string, ms: number)    persist one step's outcome
//   event(event: string)                 report step started / replayed
//   canceled(): boolean                  whether the run was asked to stop
//   start(request: string): id           start a host task; settled via __workflowSettle
//   evaluate(kind, expression, scope)    evaluate a workflow.yaml `${{ }}` expression

const host = globalThis.__workflowHost;
const TASK = Symbol("codemod.workflow.task");

const recorded = new Map();
for (const entry of JSON.parse(host.history())) recorded.set(entry.position, entry);

const pending = new Map();
const state = new Map();
let nextPosition = 0;

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

// ------------------------------------------------------------------ state ---

/** Read workflow state. Values are frozen. */
export function getState(key) {
  return state.get(key);
}

/** Set workflow state. `undefined` removes the key. */
export function setState(key, value) {
  if (value === undefined) {
    state.delete(key);
    return;
  }
  checkJson(value, `state.${key}`);
  state.set(key, freeze(JSON.parse(JSON.stringify(value))));
}

export function unsetState(key) {
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

// ------------------------------------------------------------------ tasks ---

/** Inserted by the build in place of an extracted callback. */
export function __callback(id, captures) {
  return { __flowCallback: id, captures };
}

export function forEachFile(selector, callback) {
  if (!callback || typeof callback.__flowCallback !== "string") {
    throw new TypeError(
      "forEachFile: the callback was not extracted by the build; run the workflow through the codemod-workflow build",
    );
  }
  return Object.freeze({
    [TASK]: "forEachFile",
    selector: normalizeSelector(selector),
    callback: callback.__flowCallback,
    captures: callback.captures,
  });
}

function hostTask(kind, options) {
  checkJson(options, kind);
  return Object.freeze({ [TASK]: "host", content: { ...options, kind } });
}

/** Run a shell command. */
export function shell(command, options = {}) {
  return hostTask("shell", { ...options, command });
}

/** Run a JSSG transform file over the target. */
export function jssg(path, options = {}) {
  return hostTask("jssg", { ...options, path });
}

/** Apply an ast-grep YAML rule file. */
export function astGrep(path, options = {}) {
  return hostTask("astGrep", { ...options, path });
}

/** Split files into shards. */
export function shard(options) {
  return hostTask("shard", options);
}

/** Run another codemod package (registry name or local path). */
export function codemod(source, options = {}) {
  return hostTask("codemod", { ...options, source });
}

/** Hand a prompt to a coding agent. */
export function agent(prompt, options = {}) {
  return hostTask("agent", { ...options, prompt });
}

/** Install an agent skill package. */
export function installSkill(options) {
  return hostTask("installSkill", options);
}

/** Wait for a person to approve before continuing. */
export function approval(message) {
  return hostTask("approval", { message });
}

/** Check out a branch for the following changes (managed git only). */
export function branch(name) {
  return hostTask("branch", { name });
}

/** Commit changes (managed git only). */
export function commit(message, options = {}) {
  return hostTask("commit", { ...options, message });
}

/** Push the branch and open a pull request (managed git only). */
export function pullRequest(options) {
  return hostTask("pullRequest", options);
}

export function parse() {
  throw new Error("parse(file) can only be called inside a forEachFile callback");
}

/** Evaluate a workflow.yaml `${{ }}` expression; used by translated workflows. */
export function __evaluate(kind, expression, scope) {
  return JSON.parse(host.evaluate(kind, expression, JSON.stringify(scope)));
}

// ------------------------------------------------------------------- step ---

/**
 * Run a task as a durable step. Starts immediately; the returned promise
 * resolves to the step's frozen result.
 */
export function step(labelOrTask, maybeTask) {
  const position = nextPosition++;
  const [label, task] =
    typeof labelOrTask === "string" ? [labelOrTask, maybeTask] : [undefined, labelOrTask];
  return runStep(position, label, task).then(freeze);
}

async function runStep(position, label, task) {
  // Everything up to the first await runs synchronously inside step(), so
  // captures and state are a snapshot taken when the step is called.
  let content;
  if (typeof task === "function") {
    content = { kind: "function", source: String(task) };
  } else if (task && task[TASK] === "forEachFile") {
    const captures = task.captures();
    checkJson(captures, "captures");
    content = {
      kind: "forEachFile",
      selector: task.selector,
      callback: task.callback,
      captures,
    };
  } else if (task && task[TASK] === "host") {
    content = task.content;
  } else {
    throw new TypeError(
      `step #${position}: expected a task such as forEachFile(...) or a function`,
    );
  }

  const name = label ?? defaultLabel(position, content);
  const key = canonical(content);
  const prior = recorded.get(position);
  if (prior && prior.status === "succeeded") {
    if (prior.key !== key) {
      throw new NondeterminismError(
        `step #${position} ('${name}') differs from the recorded run; the workflow changed before this step, so its recorded result cannot be reused`,
      );
    }
    applyState(prior.state);
    host.event(JSON.stringify({ type: "stepReplayed", position, label: name }));
    return prior.result;
  }

  if (host.canceled()) throw new CanceledError();
  host.event(JSON.stringify({ type: "stepStarted", position, label: name }));
  const started = Date.now();
  let result;
  let changes = null;
  try {
    if (content.kind === "function") {
      result = await task();
      if (result === undefined) result = null;
      checkJson(result, `result of step '${name}'`);
    } else {
      const outcome = await startHostTask(position, name, content);
      result = outcome.result ?? null;
      changes = outcome.state ?? null;
    }
  } catch (error) {
    if (error instanceof CanceledError || error instanceof AwaitingApprovalError) throw error;
    host.record(
      JSON.stringify({ position, label: name, key, status: "failed", error: messageOf(error) }),
      Date.now() - started,
    );
    throw new StepError(name, error);
  }
  applyState(changes);
  host.record(
    JSON.stringify({ position, label: name, key, status: "succeeded", result, state: changes }),
    Date.now() - started,
  );
  return result;
}

function startHostTask(position, label, content) {
  const id = host.start(JSON.stringify({ position, label, task: content, state: stateSnapshot() }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

globalThis.__workflowSettle = (id, ok, payload) => {
  const entry = pending.get(id);
  pending.delete(id);
  if (!entry) return;
  if (ok) {
    entry.resolve(JSON.parse(payload));
    return;
  }
  let name = "Error";
  let message = payload;
  try {
    ({ name, message } = JSON.parse(payload));
  } catch {
    // a plain message
  }
  if (name === "CanceledError") entry.reject(new CanceledError(message));
  else if (name === "AwaitingApprovalError") entry.reject(new AwaitingApprovalError(message));
  else entry.reject(new Error(message));
};

function normalizeSelector(selector) {
  if (typeof selector === "string" || Array.isArray(selector)) selector = { include: selector };
  if (!selector || typeof selector !== "object") {
    throw new TypeError(
      "forEachFile: the selector must be a glob, an array of globs, or an object",
    );
  }
  const known = new Set(["include", "exclude", "language", "containing"]);
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
  if (normalized.include.length === 0 && normalized.language === undefined) {
    throw new TypeError("forEachFile: the selector needs 'include' or 'language'");
  }
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
      return `forEachFile ${include.length ? include.join(", ") : language}`;
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

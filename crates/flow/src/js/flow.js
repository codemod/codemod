// The `codemod:flow` module, as seen by the workflow's own context.
//
// The host installs `globalThis.__flowHost` before this module evaluates:
//   history(): string        recorded steps from earlier runs, as JSON
//   record(entry: string)    persist one step's outcome
//   start(task: string): id  start a host task; settled via __flowSettle

const host = globalThis.__flowHost;
const TASK = Symbol("codemod.flow.task");

const recorded = new Map();
for (const entry of JSON.parse(host.history())) recorded.set(entry.position, entry);

const pending = new Map();
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

/** Inserted by the build in place of an extracted callback. */
export function __callback(id, captures) {
  return { __flowCallback: id, captures };
}

export function forEachFile(selector, callback) {
  if (!callback || typeof callback.__flowCallback !== "string") {
    throw new TypeError(
      "forEachFile: the callback was not extracted by the build; run the workflow through the codemod-flow build",
    );
  }
  return Object.freeze({
    [TASK]: "forEachFile",
    selector: normalizeSelector(selector),
    callback: callback.__flowCallback,
    captures: callback.captures,
  });
}

export function parse() {
  throw new Error("parse(file) can only be called inside a forEachFile callback");
}

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
  // captures are a snapshot taken when the step is called.
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
  } else {
    throw new TypeError(`step #${position}: expected forEachFile(...) or a function`);
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
    return prior.result;
  }

  let result;
  try {
    if (content.kind === "function") {
      result = await task();
      if (result === undefined) result = null;
      checkJson(result, `result of step '${name}'`);
    } else {
      result = await startHostTask(content);
    }
  } catch (error) {
    host.record(
      JSON.stringify({ position, label: name, key, status: "failed", error: messageOf(error) }),
    );
    throw new StepError(name, error);
  }
  host.record(JSON.stringify({ position, label: name, key, status: "succeeded", result }));
  return result;
}

function startHostTask(content) {
  const id = host.start(JSON.stringify(content));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

globalThis.__flowSettle = (id, ok, payload) => {
  const entry = pending.get(id);
  pending.delete(id);
  if (!entry) return;
  if (ok) entry.resolve(JSON.parse(payload));
  else entry.reject(new Error(payload));
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
  if (content.kind === "forEachFile") {
    const { include, language } = content.selector;
    return `forEachFile ${include.length ? include.join(", ") : language}`;
  }
  return `step #${position}`;
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

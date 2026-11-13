// `codemod/host`: run a workflow in this Node.js or Bun process.
//
// The workflow module is imported here (or given already imported), so its
// body, function steps and `forEachFile` callbacks run in this process: they
// can be debugged, covered and mocked like any other code. What the engine
// owns goes through the native addon: the run store, host tasks (`jssg`,
// `shell`, `codemod`, ...), and selecting and committing files.
//
// Code here has whatever Node.js gives it: capabilities are not enforced.
// The codemod CLI runs workflows in its own sandboxed runtime instead.

import "../register.mjs";

import { isAbsolute, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { addon } from "./addon.mjs";
import { context, metricsOf, stepScope } from "./context.mjs";
import { AwaitingApprovalError, CanceledError, StepError, createRuntime } from "./orchestrator.mjs";
import { callbackId } from "./workflow.mjs";

/**
 * Run a workflow and resolve with its report, for every outcome.
 *
 * `workflow` is a workflow file, a package directory, or the workflow module
 * itself (`await import("./workflow.ts")`), for test runners that should load
 * it through their own module system. A module needs `options.path` (or
 * `options.workflowDir`) so relative task paths resolve.
 */
export async function runWorkflow(workflow, options = {}) {
  const native = addon();
  const { path, load } = locate(workflow, options);
  // A workflow that cannot be loaded is not a run: reject.
  const module = await load();
  const main = module?.default;
  if (typeof main !== "function") {
    throw new TypeError(`${path} must export a default function`);
  }
  if (options.captureConsole) captureConsole();
  const run = native.Run.open({
    workflow: path,
    target: resolve(options.target ?? "."),
    params: options.params ?? {},
    name: options.name,
    dryRun: options.dryRun ?? false,
    until: options.until,
    given: options.given,
    approve: options.approve ?? true,
    capabilities: options.capabilities ?? [],
    store: options.store,
    resume: options.resume,
  });

  const flight = inFlight();
  const runtime = createRuntime(host(run, flight));
  const scope = { run, runtime, captureConsole: options.captureConsole ?? false };
  const abort = () => run.cancel();
  options.signal?.addEventListener?.("abort", abort, { once: true });

  let ending;
  try {
    const input = { params: run.params, target: run.target, dryRun: run.dryRun };
    const output = await Promise.race([context.run(scope, () => main(input)), flight.stopped]);
    ending = { status: "succeeded", output: toJson(output) };
  } catch (error) {
    ending = endingOf(error, run);
  } finally {
    options.signal?.removeEventListener?.("abort", abort);
  }
  const report = run.finish(JSON.stringify(ending));
  return { ...report, events: run.takeEvents() };
}

/**
 * Route `console` output of code that runs in a run started with
 * `captureConsole` to the run's logs, by step. Other code is untouched.
 */
function captureConsole() {
  const key = Symbol.for("codemod.host.console");
  if (globalThis[key]) return;
  globalThis[key] = true;
  for (const method of ["log", "info", "debug", "warn", "error"]) {
    const original = console[method].bind(console);
    console[method] = (...items) => {
      const store = context.getStore();
      if (!store?.captureConsole) return original(...items);
      store.run.log(store.step?.position ?? null, items.map(formatLogItem).join(" "));
    };
  }
}

function formatLogItem(value) {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

/** Where the workflow is, and how to load it. */
function locate(workflow, options) {
  if (workflow && typeof workflow === "object" && !isUrl(workflow)) {
    const path = options.path ?? options.workflowDir;
    if (!path) {
      throw new TypeError(
        "runWorkflow(module, { path }): give the workflow file's path, so relative task paths resolve",
      );
    }
    const file = resolve(String(path));
    return { path: file, load: async () => workflow };
  }
  const source = isUrl(workflow) ? fileURLToPath(workflow.href) : String(workflow);
  const path = addon().resolveWorkflow(
    isAbsolute(source) ? source : resolve(source),
    options.workflowName ?? null,
  );
  if (!/\.m?ts$|\.m?js$/.test(path)) {
    throw new Error(
      `${path} is not a TypeScript workflow; run YAML workflows with the codemod CLI, or upgrade them with \`codemod workflow upgrade\``,
    );
  }
  return { path, load: () => import(pathToFileURL(path).href) };
}

function isUrl(value) {
  return value !== null && typeof value === "object" && typeof value.href === "string";
}

/** Tracks host work in flight, to end a test run stopped at its `until` step. */
function inFlight() {
  let count = 0;
  let stopped = false;
  let finish;
  const flight = {
    stopped: new Promise((resolvePromise) => (finish = resolvePromise)),
    begin() {
      count++;
    },
    end() {
      count--;
      flight.check();
    },
    stop() {
      stopped = true;
      // Let the until step itself start before checking.
      queueMicrotask(() => setImmediate(() => flight.check()));
    },
    check() {
      if (stopped && count === 0) finish(null);
    },
  };
  return flight;
}

function host(run, flight) {
  return {
    history: () => run.history(),
    record: (entry, ms) => run.record(JSON.stringify(entry), ms),
    event: (event) => run.event(JSON.stringify(event)),
    canceled: () => run.canceled(),
    now: () => Date.now(),
    advanceClock() {},
    testPlan: () => run.testPlan(),
    stop() {
      run.stop();
      flight.stop();
    },
    evaluate: (kind, expression, scope) => run.evaluate(kind, expression, JSON.stringify(scope)),
    callbackId,
    async start(request) {
      const { fn, ...task } = request;
      flight.begin();
      try {
        if (fn) return await runInPlace(run, task, fn);
        try {
          return await run.runTask(JSON.stringify(task));
        } catch (error) {
          throw payloadOf(error);
        }
      } finally {
        flight.end();
      }
    },
  };
}

/** A host task rejected with `{ name, message }` as JSON in the message. */
function payloadOf(error) {
  try {
    const payload = JSON.parse(error.message);
    if (payload && typeof payload.message === "string") return payload;
  } catch {
    // a plain message
  }
  return { name: "Error", message: error?.message ?? String(error) };
}

// ----------------------------------------------------------- in place ---

async function runInPlace(run, task, fn) {
  const outer = context.getStore();
  if (task.task.kind === "function") {
    const step = stepScope(task.position, task.label, task.state);
    const value = await context.run({ ...outer, step }, () => fn());
    const result = value === undefined ? null : value;
    checkJson(result, "the step's result");
    flushMetrics(run, step);
    return outcome(result, step);
  }
  if (task.task.kind === "forEachFile") return forEachFile(run, outer, task, fn);
  throw new Error(`cannot run a ${task.task.kind} task in place`);
}

function outcome(result, step) {
  const state = Object.keys(step.changes).length ? step.changes : undefined;
  return state ? { result, state } : { result };
}

function flushMetrics(run, step) {
  const metrics = metricsOf(step);
  if (Object.keys(metrics).length) run.addMetrics(JSON.stringify(metrics));
  return metrics;
}

async function forEachFile(run, outer, task, fn) {
  const { selector } = task.task;
  const { language, files } = await run.selectFiles(JSON.stringify(selector));
  const step = stepScope(task.position, task.label, task.state, language ?? undefined);
  const total = files.length;
  const every = Math.max(1, Math.floor(total / 100));
  let processed = 0;
  const outputs = Array.from({ length: total });
  const failures = [];
  const effects = [];

  const one = async (index) => {
    const [path, text] = files[index];
    const done = [];
    let open = true;
    const guard = (name) => {
      if (!open) throw new Error(`file.${name}() was called after the callback finished`);
    };
    const file = Object.freeze({
      path,
      text,
      language: step.language,
      write(next) {
        guard("write");
        if (typeof next !== "string") throw new TypeError("file.write(text) expects a string");
        done.push({ op: "write", text: next });
      },
      rename(to) {
        guard("rename");
        if (typeof to !== "string" || to === "") {
          throw new TypeError("file.rename(path) expects a non-empty path");
        }
        done.push({ op: "rename", to });
      },
      delete() {
        guard("delete");
        done.push({ op: "delete" });
      },
    });
    try {
      const output = await context.run({ ...outer, step }, () => fn(file));
      if (output !== undefined) {
        checkJson(output, "return value");
        outputs[index] = { output };
      }
      if (done.length) effects.push({ path, original: text, effects: done });
    } catch (error) {
      failures.push({ path, error: describe(error) });
    } finally {
      open = false;
      processed++;
      if (processed === total || processed % every === 0) {
        run.progress(task.position, processed, total);
      }
    }
  };

  const concurrency = selector.concurrency ?? 1;
  let next = 0;
  const worker = async () => {
    while (next < total) {
      if (run.canceled()) throw new CanceledError();
      await one(next++);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, total) }, worker));

  effects.sort((a, b) => (a.path < b.path ? -1 : 1));
  const committed = await run.commitFiles(task.position, task.label, JSON.stringify(effects));
  const allFailures = [...failures, ...committed.failures].sort((a, b) =>
    a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
  );
  const metrics = flushMetrics(run, step);
  return outcome(
    {
      outputs: outputs.filter(Boolean).map((entry) => entry.output),
      changes: committed.changes,
      failures: allFailures,
      metrics,
    },
    step,
  );
}

function describe(error) {
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

// -------------------------------------------------------------- ending ---

function endingOf(error, run) {
  if (error instanceof CanceledError || error?.name === "CanceledError") {
    return { status: "canceled" };
  }
  if (error instanceof AwaitingApprovalError || error?.name === "AwaitingApprovalError") {
    return { status: "awaiting-approval", message: error.message };
  }
  if (run.canceled()) return { status: "canceled" };
  const message =
    error instanceof StepError || error?.name
      ? `${error.name ?? "Error"}: ${error.message}`
      : String(error);
  return { status: "failed", error: message };
}

function toJson(value) {
  if (value === undefined) return null;
  return JSON.parse(JSON.stringify(value));
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

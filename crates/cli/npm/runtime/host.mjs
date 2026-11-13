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

import { readFileSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { addon } from "./addon.mjs";
import { context, metricsOf, stateChanges, stepScope } from "./context.mjs";
import {
  AwaitingApprovalError,
  CanceledError,
  checkJson,
  createRuntime,
  describeError,
  fileHandle,
} from "./orchestrator.mjs";
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
  checkAddon(native);
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
  const runtime = createRuntime(host(run, flight, () => runtime));
  flight.closed = () => runtime.isClosed();
  const scope = { run, runtime, captureConsole: options.captureConsole ?? false };
  const abort = () => run.cancel();
  if (options.signal?.aborted) abort();
  options.signal?.addEventListener?.("abort", abort, { once: true });

  // How the body ended; `null` when it cannot end any more.
  let body;
  try {
    const input = { params: run.params, target: run.target, dryRun: run.dryRun };
    const ended = context
      .run(scope, () => main(input))
      .then((output) => ({ status: "succeeded", detail: toJson(output) }));
    body = await Promise.race([ended, flight.stalled]);
  } catch (error) {
    body = endingOf(error);
  }
  // Steps still running finish and are recorded before the run reports;
  // none starts from here on.
  runtime.close();
  await flight.idle();
  options.signal?.removeEventListener?.("abort", abort);
  const report = run.finish(JSON.stringify(body));
  return { ...report, events: run.takeEvents() };
}

let addonChecked = false;

/**
 * The addon and this package share the orchestrator: one from another
 * version would record and replay steps differently.
 */
function checkAddon(native) {
  if (addonChecked) return;
  const shipped = readFileSync(new URL("./orchestrator.mjs", import.meta.url), "utf8");
  if (!shipped.endsWith(native.orchestratorSource())) {
    throw new Error(
      "the codemod native addon is from another version than the codemod package; reinstall codemod so both match (in a checkout, rebuild with `cargo build -p codemod-node`)",
    );
  }
  addonChecked = true;
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
    if (!options.path && !options.workflowDir) {
      throw new TypeError(
        "runWorkflow(module, { path }): give the workflow file's path, so relative task paths resolve",
      );
    }
    // The file need not exist: only its directory matters here.
    const file = options.path
      ? resolve(String(options.path))
      : resolve(String(options.workflowDir), "workflow.ts");
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

/**
 * Tracks steps in flight. `stalled` resolves (with `null`) when the run is
 * closed and nothing is running: the body may be waiting on steps that will
 * never start, as after a test's `until` step. `idle()` resolves once nothing
 * is running.
 */
function inFlight() {
  let count = 0;
  let stall;
  let waiters = [];
  const flight = {
    closed: () => false,
    stalled: new Promise((resolvePromise) => (stall = resolvePromise)),
    begin() {
      count++;
    },
    end() {
      count--;
      flight.check();
    },
    /** Look again once the body has reacted to whatever just happened. */
    check() {
      setImmediate(() => {
        if (count !== 0) return;
        if (flight.closed()) stall(null);
        for (const waiter of waiters.splice(0)) waiter();
      });
    },
    idle() {
      return new Promise((resolvePromise) => {
        waiters.push(resolvePromise);
        flight.check();
      });
    },
  };
  return flight;
}

function host(run, flight, runtime) {
  // Replayed steps settle one per turn of the event loop, so the body reacts
  // to each before the next.
  let replaying = false;
  const replay = () => {
    replaying = runtime().replayNext();
    if (replaying) setImmediate(replay);
  };
  return {
    history: () => run.history(),
    record: (entry, ms) => run.record(JSON.stringify(entry), ms),
    event: (event) => run.event(JSON.stringify(event)),
    canceled: () => run.refusesSteps(),
    now: () => Date.now(),
    advanceClock() {},
    testPlan: () => run.testPlan(),
    stop() {
      run.stop();
      flight.check();
    },
    wake() {
      if (replaying) return;
      replaying = true;
      setImmediate(replay);
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
    let value;
    try {
      value = await context.run({ ...outer, step }, () => fn());
    } finally {
      // What the step measured counts even when it then fails.
      flushMetrics(run, step);
    }
    // A failure reported with failStep() stands even if the code caught it.
    if (step.failure !== undefined) throw new Error(step.failure);
    const result = value === undefined ? null : value;
    checkJson(result, "the step's result");
    return outcome(result, step);
  }
  if (task.task.kind === "forEachFile") return forEachFile(run, outer, task, fn);
  throw new Error(`cannot run a ${task.task.kind} task in place`);
}

function outcome(result, step) {
  const state = stateChanges(step);
  return Object.keys(state).length ? { result, state } : { result };
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
  const concurrency = selector.concurrency ?? 1;

  const one = async (index) => {
    const [path, text] = files[index];
    const { file, effects: done, close } = fileHandle(path, text, step.language);
    try {
      step.failure = undefined;
      const output = await context.run({ ...outer, step }, () => fn(file));
      // A failure reported with failFile() stands even if the callback
      // caught it. With files in flight together it cannot be told whose it
      // was, as in the CLI.
      if (step.failure !== undefined && concurrency === 1) {
        failures.push({ path, error: step.failure });
        return;
      }
      if (output !== undefined) {
        checkJson(output, "return value");
        outputs[index] = { output };
      }
      if (done.length) effects.push({ path, original: text, effects: done });
    } catch (error) {
      failures.push({ path, error: describeError(error) });
    } finally {
      close();
      processed++;
      if (processed === total || processed % every === 0) {
        run.progress(task.position, processed, total);
      }
    }
  };

  let next = 0;
  let sinceTurn = 0;
  const worker = async () => {
    while (next < total) {
      // Callbacks that never wait would keep the event loop to themselves:
      // give it a turn now and then, so a cancel can arrive.
      if (++sinceTurn >= 64) {
        sinceTurn = 0;
        await new Promise((turn) => setImmediate(turn));
      }
      if (run.refusesSteps()) throw new CanceledError();
      await one(next++);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, total) }, worker));

  // Before the commit: a commit that fails must not lose what was measured.
  const metrics = flushMetrics(run, step);
  effects.sort((a, b) => (a.path < b.path ? -1 : 1));
  const committed = await run.commitFiles(task.position, task.label, JSON.stringify(effects));
  const allFailures = [...failures, ...committed.failures].sort((a, b) =>
    a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
  );
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

// -------------------------------------------------------------- ending ---

/** How a body that threw ended. */
function endingOf(error) {
  if (error instanceof CanceledError || error?.name === "CanceledError") {
    return { status: "canceled" };
  }
  if (error instanceof AwaitingApprovalError || error?.name === "AwaitingApprovalError") {
    return { status: "awaiting-approval", detail: error.message };
  }
  const message =
    error && typeof error === "object" && "message" in error
      ? `${error.name ?? "Error"}: ${error.message}`
      : String(error);
  return { status: "failed", detail: message };
}

/** A value as JSON keeps it; what JSON has no form for (a function) is null. */
function toJson(value) {
  const text = JSON.stringify(value);
  return text === undefined ? null : JSON.parse(text);
}

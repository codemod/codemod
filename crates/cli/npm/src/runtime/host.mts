// `codemod/host`: run a workflow in this Node.js or Bun process.
//
// The workflow module is imported here (or given already imported), so its
// body, function steps and `forEachFile` callbacks run in this process: they
// can be debugged, covered and mocked like any other code. What the engine
// owns goes through the native addon: the run store, host tasks (`shell`,
// `astGrep`, `shard`, `codemod`, ...), and selecting and committing files.
//
// Code here has whatever Node.js gives it: capabilities are not enforced.
// The codemod CLI runs workflows in its own sandboxed runtime instead.

/**
 * Run a workflow in this Node.js or Bun process.
 *
 * ```ts
 * import { runWorkflow } from "codemod/host";
 *
 * const report = await runWorkflow("./my-codemod", { target: "./repo" });
 * if (report.status !== "succeeded") throw new Error(report.error ?? report.status);
 * ```
 *
 * The workflow's body, function steps and `forEachFile` callbacks run here,
 * so they can be debugged, covered and mocked. Host tasks (`shell`,
 * `astGrep`, `shard`, `codemod`, ...) and the run store go through the
 * native addon; `shard`'s functions run in the engine's isolates.
 *
 * Code here has whatever Node.js gives it: the capabilities codemod.yaml
 * declares are not enforced. The codemod CLI runs workflows in its own
 * sandboxed runtime instead; run packages from the registry with the CLI.
 *
 * @module
 */

import "../register.mts";

import { isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import type { JsonValue } from "../testing/index.mts";
import { addon } from "./addon.mts";
import type { Addon, FileEffects, NativeRun } from "./addon.mts";
import { context, stepScope } from "./context.mts";
import type { RunScope } from "./context.mts";
import { ORCHESTRATOR_VERSION } from "./orchestrator-version.mts";
import {
  AwaitingApprovalError,
  CanceledError,
  checkJson,
  createRuntime,
  describeDuration,
  describeError,
  fileHandle,
} from "./orchestrator.mts";
import type {
  Callback,
  Host,
  Json,
  Outcome,
  Runtime,
  Selector,
  StartRequest,
} from "./orchestrator.mts";
import { workflowUrl } from "./transforms.mts";
import { callbackId } from "./workflow.mts";

/** A `URL` (`new URL("./workflow.ts", import.meta.url)`); only `file:` URLs are paths. */
export interface UrlLike {
  readonly href: string;
}

/** An `AbortSignal`, or what a test environment has in its place. */
export interface AbortSignalLike {
  readonly aborted: boolean;
  addEventListener?(type: "abort", listener: () => void, options?: { once?: boolean }): void;
  removeEventListener?(type: "abort", listener: () => void): void;
}

/** A workflow module: what `import("./workflow.ts")` gives. */
export interface WorkflowModule {
  default: (context: {
    params: Record<string, JsonValue>;
    target: string;
    dryRun: boolean;
    platform: string;
  }) => unknown;
}

export interface HostRunOptions {
  /** The directory the workflow changes. Default: the current directory. */
  target?: string;
  /** The workflow's parameters. */
  params?: Record<string, JsonValue>;
  /** A name for the run, where runs are listed. */
  name?: string;
  /** Run every step but change no files. */
  dryRun?: boolean;
  /** Stop after the step with this label (a test run). */
  until?: string;
  /** Results for steps that should not run, by label (a test run). */
  given?: Record<string, JsonValue>;
  /** Give every approval the workflow asks for. Default `true`. */
  approve?: boolean;
  /** Capabilities for host tasks' transforms, on top of those codemod.yaml declares. */
  capabilities?: ("fs" | "fetch" | "child_process")[];
  /** Which workflow to run when codemod.yaml defines several. */
  workflowName?: string;
  /**
   * `"memory"` (the default) keeps nothing; `"local"` records the run where
   * `codemod workflow list` and `status` show it.
   */
  store?: "memory" | "local";
  /**
   * Continue this run (with `store: "local"`). A run is resumed by the host
   * that started it: one started here cannot be resumed with
   * `codemod workflow resume`, nor one the CLI started here.
   */
  resume?: string;
  /** The workflow file, when `workflow` is the module itself. */
  path?: string;
  /** Instead of `path`: the directory relative task paths resolve against. */
  workflowDir?: string;
  /** Send `console` output of the workflow's code to the report's events. */
  captureConsole?: boolean;
  /** Stops the run. */
  signal?: AbortSignalLike;
}

export interface HostStepReport {
  readonly position: number;
  readonly label: string;
  /** `"partial"`: the step finished, but some of its files failed. */
  readonly status: "succeeded" | "partial" | "failed";
  readonly result: JsonValue | null;
  readonly error: string | null;
  readonly given: boolean;
}

export interface HostRunReport {
  readonly runId: string;
  readonly status: "succeeded" | "failed" | "canceled" | "awaiting-approval" | "stopped";
  readonly output: JsonValue | null;
  readonly error: string | null;
  /** The approval the run waits for. */
  readonly awaiting: string | null;
  /** The `until` step the run stopped after. */
  readonly stoppedAt: string | null;
  readonly steps: readonly HostStepReport[];
  readonly stats: {
    readonly filesModified: number;
    readonly filesUnmodified: number;
    readonly filesWithErrors: number;
  };
  /** What the engine reported while the run progressed. */
  readonly events: readonly ({ readonly event: string } & Record<string, unknown>)[];
}

/**
 * How the workflow's body ended, as `Run.finish` takes it (`Ending` in
 * crates/workflow/src/run.rs); `null` when it cannot end any more.
 */
type BodyEnding =
  | { status: "succeeded"; detail: Json }
  | { status: "failed" | "awaiting-approval"; detail: string }
  | { status: "canceled" }
  | null;

/**
 * Run a workflow (a workflow file, a package directory, or the workflow
 * module itself). Resolves for every outcome; rejects when the workflow
 * cannot be loaded. YAML workflows run only with the CLI.
 *
 * A module (`await import("./workflow.ts")`), for test runners that should
 * load it through their own module system, needs `options.path` (or
 * `options.workflowDir`) so relative task paths resolve. Such a module (and
 * any workflow on Bun) is loaded without the build's changes: imported
 * callbacks load with it and run as plain functions, and `shard`'s
 * `method.function` and `filter` fail. Give those workflows by path.
 */
export async function runWorkflow(
  workflow: string | UrlLike | WorkflowModule,
  options: HostRunOptions = {},
): Promise<HostRunReport> {
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
  const runtime: Runtime = createRuntime(host(run, flight, () => runtime));
  flight.closed = () => runtime.isClosed();
  const scope: RunScope = { run, runtime, captureConsole: options.captureConsole ?? false };
  const abort = () => run.cancel();
  if (options.signal?.aborted) abort();
  options.signal?.addEventListener?.("abort", abort, { once: true });

  let body: BodyEnding;
  try {
    const input = {
      params: run.params,
      target: run.target,
      dryRun: run.dryRun,
      platform: process.platform,
    };
    const ended = context
      .run(scope, async () => main(input))
      .then((output): BodyEnding => ({ status: "succeeded", detail: toJson(output) }));
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
function checkAddon(native: Addon): void {
  if (addonChecked) return;
  // An addon from before the check by version has no `orchestratorVersion`.
  const built =
    typeof native.orchestratorVersion === "function" ? native.orchestratorVersion() : undefined;
  if (built !== ORCHESTRATOR_VERSION) {
    throw new Error(
      "the codemod native addon is from another version than the codemod package; reinstall codemod so both match (in a checkout, rebuild both: `cargo build -p codemod-node` and `pnpm --filter codemod build`)",
    );
  }
  addonChecked = true;
}

const CONSOLE_METHODS = ["log", "info", "debug", "warn", "error"] as const;

/**
 * Route `console` output of code that runs in a run started with
 * `captureConsole` to the run's logs, by step. Other code is untouched.
 */
function captureConsole(): void {
  const key = Symbol.for("codemod.host.console");
  const shared = globalThis as { [key: symbol]: unknown };
  if (shared[key]) return;
  shared[key] = true;
  for (const method of CONSOLE_METHODS) {
    const original = console[method].bind(console);
    console[method] = (...items: unknown[]) => {
      const store = context.getStore();
      if (!store?.captureConsole) return original(...items);
      store.run.log(store.step?.position ?? null, items.map(formatLogItem).join(" "));
    };
  }
}

function formatLogItem(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

/** Where the workflow is, and how to load it. */
function locate(
  workflow: string | UrlLike | WorkflowModule,
  options: HostRunOptions,
): { path: string; load(): Promise<Partial<WorkflowModule> | undefined> } {
  if (typeof workflow !== "string" && !isUrl(workflow)) {
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
  // Marked for the load hook, which applies the build's changes to what
  // the engine runs (see transforms.mts).
  return { path, load: () => import(workflowUrl(path)) };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object";
}

// Not `instanceof URL`: test environments may have their own URL class.
function isUrl(value: unknown): value is UrlLike {
  return isRecord(value) && typeof value.href === "string";
}

/** Steps in flight. */
interface Flight {
  /** Whether the run is closed: set once the runtime exists. */
  closed: () => boolean;
  /**
   * Resolves (with `null`) when the run is closed and nothing is running:
   * the body may be waiting on steps that will never start, as after a
   * test's `until` step.
   */
  readonly stalled: Promise<null>;
  begin(): void;
  end(): void;
  /** Look again once the body has reacted to whatever just happened. */
  check(): void;
  /** Resolves once nothing is running. */
  idle(): Promise<void>;
}

function inFlight(): Flight {
  let count = 0;
  let stall: (ending: null) => void = () => {};
  const waiters: (() => void)[] = [];
  const flight: Flight = {
    closed: () => false,
    stalled: new Promise<null>((resolvePromise) => (stall = resolvePromise)),
    begin() {
      count++;
    },
    end() {
      count--;
      flight.check();
    },
    check() {
      setImmediate(() => {
        if (count !== 0) return;
        if (flight.closed()) stall(null);
        for (const waiter of waiters.splice(0)) waiter();
      });
    },
    idle() {
      return new Promise<void>((resolvePromise) => {
        waiters.push(resolvePromise);
        flight.check();
      });
    },
  };
  return flight;
}

function host(run: NativeRun, flight: Flight, runtime: () => Runtime): Host {
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
        if (fn) return await within(task.timeoutMs, (limit) => runInPlace(run, task, fn, limit));
        return await runNative(run, task);
      } finally {
        flight.end();
      }
    },
  };
}

async function runNative(run: NativeRun, task: InPlaceTask): Promise<Outcome> {
  try {
    return await run.runTask(JSON.stringify(task));
  } catch (error) {
    throw payloadOf(error);
  }
}

/** A host task rejected with `{ name, message }` as JSON in the message. */
function payloadOf(error: unknown): { name: string; message: string } {
  const message = isRecord(error) && typeof error.message === "string" ? error.message : undefined;
  try {
    const payload: unknown = JSON.parse(message ?? "");
    if (isRecord(payload) && typeof payload.message === "string") {
      return { name: String(payload.name ?? "Error"), message: payload.message };
    }
  } catch {
    // a plain message
  }
  return { name: "Error", message: message ?? String(error) };
}

// ----------------------------------------------------------- in place ---

/** A task this process runs itself: its request, without the function. */
type InPlaceTask = Omit<StartRequest, "fn">;

/** Set when a step's time is up, for work that can stop between two things. */
interface Limit {
  expired: boolean;
  /** Also checks elapsed time when synchronous work has delayed the timer. */
  check(): void;
}

/**
 * Run `work` for at most `ms`. JavaScript that is running cannot be stopped
 * from outside: when the time is up the step fails, `limit.expired` tells
 * the work to stop where it can, and whatever it still does is discarded.
 */
async function within<T>(ms: number | undefined, work: (limit: Limit) => Promise<T>): Promise<T> {
  const started = performance.now();
  const timeout = (ms: number) => ({
    name: "Error",
    message: `timed out after ${describeDuration(ms)}`,
  });
  const limit: Limit = {
    expired: false,
    check() {
      if (ms !== undefined && (limit.expired || performance.now() - started >= ms))
        throw timeout(ms);
    },
  };
  if (ms === undefined) return work(limit);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      limit.expired = true;
      reject(timeout(ms));
    }, ms);
  });
  try {
    return await Promise.race([work(limit), late]);
  } finally {
    clearTimeout(timer);
  }
}

async function runInPlace(
  run: NativeRun,
  task: InPlaceTask,
  fn: Callback,
  limit: Limit,
): Promise<Outcome> {
  const outer = context.getStore();
  if (!outer) throw new Error("a step started outside its run");
  if (task.task.kind === "function") {
    const step = stepScope(task.position, task.label);
    const value = await context.run({ ...outer, step }, () => fn());
    // A failure reported with failStep() stands even if the code caught it.
    if (step.failure !== undefined) throw new Error(step.failure);
    const result = value === undefined ? null : value;
    checkJson(result, "the step's result");
    return { result };
  }
  if (task.task.kind === "forEachFile") return forEachFile(run, outer, task, fn, limit);
  throw new Error(`cannot run a ${task.task.kind} task in place`);
}

async function forEachFile(
  run: NativeRun,
  outer: RunScope,
  task: InPlaceTask,
  fn: Callback,
  limit: Limit,
): Promise<Outcome> {
  const selector = task.task.selector as Selector;
  const { language, files } = await run.selectFiles(JSON.stringify(selector));
  const step = stepScope(task.position, task.label, language ?? undefined);
  const total = files.length;
  const every = Math.max(1, Math.floor(total / 100));
  let processed = 0;
  const outputs: ({ output: Json } | undefined)[] = Array.from({ length: total });
  const failures: { path: string; error: string }[] = [];
  const effects: FileEffects[] = [];
  const concurrency = selector.concurrency ?? 1;

  const one = async (index: number, [path, text]: [path: string, text: string]) => {
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
    for (;;) {
      const index = next++;
      const file = files[index];
      if (!file) return;
      // Callbacks that never wait would keep the event loop to themselves:
      // give it a turn now and then, so a cancel can arrive.
      if (++sinceTurn >= 64) {
        sinceTurn = 0;
        await new Promise((turn) => setImmediate(turn));
      }
      if (run.refusesSteps()) throw new CanceledError();
      if (limit.expired) return;
      await one(index, file);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, total) }, worker));
  // Out of time: the step has already failed, and nothing it did is written.
  if (limit.expired) throw new CanceledError("the step ran out of time");

  effects.sort((a, b) => (a.path < b.path ? -1 : 1));
  const committed = await run.commitFiles(task.position, task.label, JSON.stringify(effects));
  const allFailures = [...failures, ...committed.failures].sort((a, b) =>
    a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
  );
  // As the CLI does: each file that failed, then its error.
  for (const { path, error } of allFailures) {
    run.log(task.position, path);
    for (const line of error.split("\n")) run.log(task.position, `  ${line}`);
  }
  // The step fails when the callback failed on every file, and is partial
  // when it failed on some.
  const [first] = allFailures;
  if (first && allFailures.length === total) {
    throw new Error(
      `the callback failed on every file; first error: ${first.path}: ${first.error}`,
    );
  }
  const result: Outcome = {
    result: {
      outputs: outputs.flatMap((entry) => (entry ? [entry.output] : [])),
      changes: committed.changes,
      failures: allFailures,
    },
  };
  if (!allFailures.length) return result;
  return {
    ...result,
    partial: `${allFailures.length} of ${total} ${total === 1 ? "file" : "files"} failed`,
  };
}

// -------------------------------------------------------------- ending ---

/** How a body that threw ended. */
function endingOf(error: unknown): BodyEnding {
  const name = isRecord(error) ? error.name : undefined;
  if (error instanceof CanceledError || name === "CanceledError") {
    return { status: "canceled" };
  }
  if (error instanceof AwaitingApprovalError || name === "AwaitingApprovalError") {
    return { status: "awaiting-approval", detail: String((error as Error).message) };
  }
  const message =
    isRecord(error) && "message" in error
      ? `${String(name ?? "Error")}: ${String(error.message)}`
      : String(error);
  return { status: "failed", detail: message };
}

/** A value as JSON keeps it; what JSON has no form for (a function) is null. */
function toJson(value: unknown): Json {
  const text = JSON.stringify(value);
  return text === undefined ? null : (JSON.parse(text) as Json);
}

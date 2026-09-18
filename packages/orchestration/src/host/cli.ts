/**
 * Experimental local workflow runner: `codemod-workflow <workflow.ts>`.
 *
 * Loads a TypeScript workflow module through the build step (`bundle/build.ts`:
 * inline JSSG transforms are bundled into artifacts and the module is
 * rewritten to reference them), runs its default export through the Rust
 * execution bridge (one process per `shell` or JSSG command), and prints the
 * final value as JSON. Trusted local use only: the workflow runs in plain
 * Node, not a restricted sandbox, and nothing here validates registry
 * packages. SIGINT/SIGTERM/SIGHUP abort the run: the operation in flight is
 * cancelled (bridge process tree killed, nothing written; an agent that had
 * started is `unknown`). Bridges run in their own process groups, so a closed
 * terminal only reaches them through this abort.
 *
 * `--dashboard` turns the command into a host: it serves the local run
 * dashboard (`dashboard/`), starts the first run, and stays alive after that
 * run finishes so the page's Restart, Run again, and Recent runs keep
 * working. It ends on SIGINT/SIGTERM/SIGHUP (the `signal`): the active run, if any,
 * is aborted and waited for, the server closes, and the command exits. The
 * dashboard URL and one line per run start and outcome go to `notify`
 * (stderr). stdout stays the JSON result: at exit it carries the output of
 * the newest run, and the command fails with that run's error when it did
 * not complete, exactly as a plain run would.
 */
import { existsSync } from "node:fs";
import { basename, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { loadWorkflow } from "../bundle/build.ts";
import { executableRequiresInput, isExecutable } from "../authoring/composition.ts";
import { BridgeExecutor } from "../execution/executor.ts";
import type { Json } from "../core/json.ts";
import { run } from "../runtime/run.ts";
import { startDashboard, type Dashboard } from "./dashboard/server.ts";
import { DashboardSession, type RunSummary } from "./dashboard/session.ts";

export interface CliOptions {
  /** Workflow module path. */
  workflow: string;
  /** Repository the workflow operates on; `shell` cwd and JSSG target root. */
  target: string;
  /** Path to the `butterflow-execution-bridge` binary. */
  bridge: string;
  /**
   * The root input, parsed from `--input <json>`. The key is present only
   * when the flag was given: `--input null` is the value `null`, no flag is
   * no input.
   */
  input?: Json;
  /** Host the loopback dashboard and stay alive until the signal fires. */
  dashboard: boolean;
}

/** Seams for tests: replace the socket server or observe the session. */
export interface CliHooks {
  /** Serves the session instead of `startDashboard`, e.g. where a sandbox denies `listen`. */
  serve?: (session: DashboardSession) => Promise<Dashboard>;
  /** Receives the session as soon as it exists, before the first run starts. */
  onSession?: (session: DashboardSession) => void;
}

/**
 * Signals the CLI turns into an abort of the run. Bridges run detached in
 * their own process groups, so a terminal's SIGINT or SIGHUP reaches them
 * only through this abort; hosts embedding `run()` must do the same.
 */
export const ABORT_SIGNALS = ["SIGINT", "SIGTERM", "SIGHUP"] as const;

export const USAGE =
  "usage: codemod-workflow <workflow.ts> [--target <directory>] [--bridge <binary>] [--input <json>] [--dashboard]";

/**
 * `notify` receives out-of-band lines such as the dashboard URL; the default
 * writes them to stderr so stdout remains the result. In dashboard mode the
 * promise settles only once `signal` has fired and the session has closed;
 * without a signal the host stays up until the process is killed.
 */
export async function runWorkflowCli(
  argv: string[],
  signal?: AbortSignal,
  notify: (line: string) => void = (line) => process.stderr.write(`${line}\n`),
  hooks: CliHooks = {},
): Promise<unknown> {
  const options = parseArgs(argv);
  if (!existsSync(options.workflow)) {
    throw new Error(`workflow does not exist: ${options.workflow}`);
  }
  if (!existsSync(options.target)) throw new Error(`target does not exist: ${options.target}`);
  if (!existsSync(options.bridge)) {
    throw new Error(
      `bridge binary not found at ${options.bridge}; build it with 'cargo build -p butterflow-execution-bridge' or pass --bridge / CODEMOD_BRIDGE_BIN`,
    );
  }
  const { exports, artifacts } = await loadWorkflow(options.workflow);
  const executable = exports.default;
  if (!isExecutable(executable)) {
    throw new Error(
      "workflow module must default-export a shell or jssg step, a step invocation, dynamic(...), sequence(...), or parallel(...)",
    );
  }
  const hasInput = "input" in options;
  if (!hasInput && executableRequiresInput(executable)) {
    throw new Error("workflow requires an input value; pass --input <json>");
  }
  if (!options.dashboard) {
    const result = await run(executable, {
      executor: new BridgeExecutor({ bin: options.bridge, cwd: options.target, artifacts }),
      signal,
      ...(hasInput ? { input: options.input } : {}),
    });
    return result.output;
  }

  // The session owns every run of this configuration; each run gets a fresh
  // executor, monitor, scheduler, and abort controller from it.
  const session = new DashboardSession({
    executable,
    workflow: basename(options.workflow),
    executor: (events) =>
      new BridgeExecutor({ bin: options.bridge, cwd: options.target, artifacts, events }),
    ...(hasInput ? { input: options.input } : {}),
  });
  hooks.onSession?.(session);
  const dashboard = await (hooks.serve ?? ((s) => startDashboard({ session: s })))(session);
  notify(`dashboard: ${dashboard.url}`);
  const unsubscribe = session.subscribe((notice) => {
    if (notice.type === "run.created") notify(`run ${notice.run.number} started`);
    else notify(describeOutcome(notice.run));
  });
  const shutdown = (): void => void session.close();
  if (signal?.aborted) shutdown();
  else signal?.addEventListener("abort", shutdown, { once: true });
  try {
    await session.start();
    await session.closed;
  } finally {
    signal?.removeEventListener("abort", shutdown);
    unsubscribe();
    await dashboard.close();
  }
  // The command's own result is the newest run's, as it would be without a dashboard.
  const outcome = session.outcome();
  if (outcome === undefined) throw new Error("the dashboard session closed before a run settled");
  if ("error" in outcome) throw outcome.error;
  return outcome.output;
}

function describeOutcome(run: RunSummary): string {
  const took = `${(run.durationMs / 1000).toFixed(1)}s`;
  switch (run.status) {
    case "completed":
      return `run ${run.number} done in ${took}`;
    case "failed":
      return `run ${run.number} failed after ${took}: ${run.error ?? "unknown error"}`;
    case "cancelled":
      return `run ${run.number} stopped after ${took}`;
    default:
      return `run ${run.number} ${run.status}`;
  }
}

/**
 * All paths come back absolute. The bridge defaults to the monorepo debug
 * build when the package runs from the source checkout.
 */
export function parseArgs(argv: string[]): CliOptions {
  const args = [...argv];
  const workflow = args.shift();
  if (workflow === undefined || workflow.startsWith("-")) throw new Error(USAGE);
  let target = process.cwd();
  let bridge =
    process.env.CODEMOD_BRIDGE_BIN ??
    resolve(import.meta.dirname, "../../../../target/debug/butterflow-execution-bridge");
  let input: { value: Json } | undefined;
  let dashboard = false;
  while (args.length > 0) {
    const flag = args.shift();
    if (flag === "--dashboard") {
      dashboard = true;
      continue;
    }
    const value = args.shift();
    if (value === undefined) throw new Error(`missing value for ${flag}\n${USAGE}`);
    if (flag === "--target") target = value;
    else if (flag === "--bridge") bridge = value;
    else if (flag === "--input") input = { value: parseInput(value) };
    else throw new Error(`unknown option: ${flag}\n${USAGE}`);
  }
  return {
    workflow: resolve(workflow),
    target: resolve(target),
    bridge: resolve(bridge),
    ...(input === undefined ? {} : { input: input.value }),
    dashboard,
  };
}

/** Strict JSON only (`JSON.parse`): no comments, trailing commas, or bare words. */
function parseInput(text: string): Json {
  try {
    return JSON.parse(text) as Json;
  } catch (error) {
    throw new Error(
      `--input must be valid JSON (${(error as Error).message}); got: ${text}\n${USAGE}`,
    );
  }
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const controller = new AbortController();
  const abort = () => controller.abort();
  for (const name of ABORT_SIGNALS) process.once(name, abort);
  runWorkflowCli(process.argv.slice(2), controller.signal)
    .then(
      (output) => process.stdout.write(`${JSON.stringify(output, null, 2)}\n`),
      (error: unknown) => {
        process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
        process.exitCode = 1;
      },
    )
    .finally(() => {
      for (const name of ABORT_SIGNALS) process.off(name, abort);
    });
}

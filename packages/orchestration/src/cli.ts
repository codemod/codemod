/**
 * Experimental local workflow runner: `codemod-workflow <workflow.ts>`.
 *
 * Loads a TypeScript workflow module through the build step (`build.ts`:
 * inline JSSG transforms are bundled into artifacts and the module is
 * rewritten to reference them), runs its default export through the Rust
 * execution bridge (one process per `exec` or JSSG command), and prints the
 * final value as JSON. Trusted local use only: the workflow runs in plain
 * Node, not a restricted sandbox, and nothing here validates registry
 * packages. SIGINT/SIGTERM abort the run: the operation in flight is
 * cancelled (bridge killed, nothing written).
 */
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { loadWorkflow } from "./build.ts";
import { BridgeExecutor } from "./executor.ts";
import { isPlan } from "./plan.ts";
import { run, type Executable } from "./workflow.ts";

export interface CliOptions {
  /** Workflow module path. */
  workflow: string;
  /** Repository the workflow operates on; `exec` cwd and JSSG target root. */
  target: string;
  /** Path to the `butterflow-execution-bridge` binary. */
  bridge: string;
}

export const USAGE =
  "usage: codemod-workflow <workflow.ts> [--target <directory>] [--bridge <binary>]";

export async function runWorkflowCli(argv: string[], signal?: AbortSignal): Promise<unknown> {
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
  if (!isExecutable(exports.default)) {
    throw new Error("workflow module must default-export workflow(...) or plan(...)");
  }
  const result = await run(exports.default, {
    executor: new BridgeExecutor({ bin: options.bridge, cwd: options.target, artifacts }),
    signal,
  });
  return result.output;
}

function isExecutable(value: unknown): value is Executable {
  if (isPlan(value)) return true;
  return (
    typeof value === "object" &&
    value !== null &&
    (value as { type?: unknown }).type === "workflow" &&
    typeof (value as { body?: unknown }).body === "function"
  );
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
    resolve(import.meta.dirname, "../../../target/debug/butterflow-execution-bridge");
  while (args.length > 0) {
    const flag = args.shift();
    const value = args.shift();
    if (value === undefined) throw new Error(`missing value for ${flag}\n${USAGE}`);
    if (flag === "--target") target = value;
    else if (flag === "--bridge") bridge = value;
    else throw new Error(`unknown option: ${flag}\n${USAGE}`);
  }
  return { workflow: resolve(workflow), target: resolve(target), bridge: resolve(bridge) };
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const controller = new AbortController();
  const abort = () => controller.abort();
  process.once("SIGINT", abort);
  process.once("SIGTERM", abort);
  runWorkflowCli(process.argv.slice(2), controller.signal)
    .then(
      (output) => process.stdout.write(`${JSON.stringify(output, null, 2)}\n`),
      (error: unknown) => {
        process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
        process.exitCode = 1;
      },
    )
    .finally(() => {
      process.off("SIGINT", abort);
      process.off("SIGTERM", abort);
    });
}

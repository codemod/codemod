/**
 * Experimental local workflow runner: `codemod-workflow <workflow.ts>`.
 *
 * Loads a TypeScript workflow module through the build step (`bundle/build.ts`:
 * inline JSSG transforms are bundled into artifacts and the module is
 * rewritten to reference them), runs its default export through the Rust
 * execution bridge (one process per `shell` or JSSG command), and prints the
 * final value as JSON. Trusted local use only: the workflow runs in plain
 * Node, not a restricted sandbox, and nothing here validates registry
 * packages. SIGINT/SIGTERM abort the run: the operation in flight is
 * cancelled (bridge killed, nothing written).
 */
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { loadWorkflow } from "../bundle/build.ts";
import { executableRequiresInput, isExecutable } from "../authoring/composition.ts";
import { BridgeExecutor } from "../execution/executor.ts";
import type { Json } from "../core/json.ts";
import { run } from "../runtime/run.ts";

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
}

export const USAGE =
  "usage: codemod-workflow <workflow.ts> [--target <directory>] [--bridge <binary>] [--input <json>]";

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
  const result = await run(executable, {
    executor: new BridgeExecutor({ bin: options.bridge, cwd: options.target, artifacts }),
    signal,
    ...(hasInput ? { input: options.input } : {}),
  });
  return result.output;
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
  while (args.length > 0) {
    const flag = args.shift();
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

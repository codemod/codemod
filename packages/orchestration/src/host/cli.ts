/**
 * Experimental local workflow runner:
 * `codemod-workflow [run] <workflow.ts> [--runtime quickjs]`.
 *
 * Builds the workflow's QuickJS bundle (`quickjs-bundle.ts`, cached by the
 * content of every input file), runs it on the Rust QuickJS host
 * (`runtimes/quickjs.ts`), and prints the final value as JSON. A runtime
 * that cannot run here (the host binary is missing) fails before anything is
 * built, and the command exits nonzero. SIGINT/SIGTERM/SIGHUP abort the run:
 * the operation in flight is cancelled (a shell's or agent runner's process
 * tree killed, nothing written; an agent that had started is `unknown`).
 * Child processes run in their own process groups, so a closed terminal only
 * reaches them through this abort.
 */
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import type { Json } from "../core/json.ts";
import { Phase, lap, now, setTimingsArm } from "../core/timings.ts";
import { DEFAULT_RUNTIME } from "../runtime/capabilities.ts";
import { endBoot } from "./timings-file.ts";
import { defaultRuntimes, runWorkflow, type RuntimeRegistry } from "./runtimes/registry.ts";

export interface CliOptions {
  /** Workflow module path. */
  workflow: string;
  /** Repository the workflow operates on; `shell` cwd and JSSG target root. */
  target: string;
  /**
   * `--runtime <name>` (`quickjs`, or any registered runtime).
   * Present only when the flag was given; the default is `quickjs`.
   */
  runtime?: string;
  /**
   * The root input, parsed from `--input <json>`. The key is present only
   * when the flag was given: `--input null` is the value `null`, no flag is
   * no input.
   */
  input?: Json;
}

/** Seams for tests. */
export interface CliHooks {
  /** Runtimes `--runtime` resolves against. Default `defaultRuntimes`. */
  registry?: RuntimeRegistry;
}

/**
 * Signals the CLI turns into an abort of the run. Child processes run
 * detached in their own process groups, so a terminal's SIGINT or SIGHUP
 * reaches them only through this abort; hosts embedding `run()` must do the
 * same.
 */
export const ABORT_SIGNALS = ["SIGINT", "SIGTERM", "SIGHUP"] as const;

export const USAGE = `usage: codemod-workflow [run] <workflow.ts> [--runtime <${defaultRuntimes.names().join("|")}>] [--target <directory>] [--input <json>]
       codemod-workflow build <workflow.ts> --runtime quickjs -o <bundle.mjs>`;

/**
 * `codemod-workflow build <workflow.ts> --runtime=quickjs -o <bundle.mjs>`:
 * write the self-contained QuickJS bundle (`quickjs-bundle.ts`) and return a
 * summary (`{ bundle, hash, artifacts }`). Run it with
 * `orchestration-quickjs-host run <bundle.mjs> --target <directory>`.
 */
export async function buildBundleCli(argv: string[]): Promise<unknown> {
  const args = argv.flatMap((arg) => {
    const match = /^(--[^=]+)=(.*)$/su.exec(arg);
    return match ? [match[1]!, match[2]!] : [arg];
  });
  const workflow = args.shift();
  if (workflow === undefined || workflow.startsWith("-")) throw new Error(USAGE);
  let runtime: string | undefined;
  let out: string | undefined;
  while (args.length > 0) {
    const flag = args.shift();
    const value = args.shift();
    if (value === undefined) throw new Error(`missing value for ${flag}\n${USAGE}`);
    if (flag === "--runtime") runtime = value;
    else if (flag === "-o" || flag === "--out") out = value;
    else throw new Error(`unknown option: ${flag}\n${USAGE}`);
  }
  if (runtime !== "quickjs") {
    throw new Error(
      `build produces bundles for --runtime quickjs only${runtime === undefined ? "" : ` (got '${runtime}')`}\n${USAGE}`,
    );
  }
  if (out === undefined) throw new Error(`missing -o <bundle.mjs>\n${USAGE}`);
  const { buildQuickjsBundle } = await import("./quickjs-bundle.ts");
  const bundle = await buildQuickjsBundle(resolve(workflow));
  const { mkdirSync, writeFileSync } = await import("node:fs");
  const { dirname } = await import("node:path");
  const target = resolve(out);
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, bundle.code);
  return {
    bundle: target,
    hash: bundle.hash,
    artifacts: bundle.artifacts.map(({ name, hash }) => ({ name, hash })),
  };
}

/** `notify` receives out-of-band lines; the default writes them to stderr so stdout remains the result. */
export async function runWorkflowCli(
  argv: string[],
  signal?: AbortSignal,
  notify: (line: string) => void = (line) => process.stderr.write(`${line}\n`),
  hooks: CliHooks = {},
): Promise<unknown> {
  if (argv[0] === "build") return buildBundleCli(argv.slice(1));
  const options = parseArgs(argv);
  setTimingsArm(`ts-${options.runtime ?? DEFAULT_RUNTIME}`);
  return runWorkflow(options.workflow, {
    target: options.target,
    notify,
    ...(options.runtime === undefined ? {} : { runtime: options.runtime }),
    ...(hooks.registry === undefined ? {} : { registry: hooks.registry }),
    ...(signal === undefined ? {} : { signal }),
    ...("input" in options ? { input: options.input } : {}),
  });
}

/** All paths come back absolute. */
export function parseArgs(argv: string[]): CliOptions {
  // `--flag=value` is the same as `--flag value`.
  const args = argv.flatMap((arg) => {
    const match = /^(--[^=]+)=(.*)$/su.exec(arg);
    return match ? [match[1]!, match[2]!] : [arg];
  });
  // `run` is the (optional) subcommand: `codemod-workflow run <workflow.ts>`.
  if (args[0] === "run") args.shift();
  // The workflow is the one positional argument; options may come before it
  // (`run --runtime=quickjs <workflow.ts>`) or after it.
  let workflow: string | undefined;
  let target = process.cwd();
  let input: { value: Json } | undefined;
  let runtime: string | undefined;
  while (args.length > 0) {
    const flag = args.shift()!;
    if (!flag.startsWith("-")) {
      if (workflow !== undefined) throw new Error(`unexpected argument: ${flag}\n${USAGE}`);
      workflow = flag;
      continue;
    }
    const value = args.shift();
    if (value === undefined) throw new Error(`missing value for ${flag}\n${USAGE}`);
    if (flag === "--target") target = value;
    else if (flag === "--input") input = { value: parseInput(value) };
    else if (flag === "--runtime") runtime = value;
    else throw new Error(`unknown option: ${flag}\n${USAGE}`);
  }
  if (workflow === undefined) throw new Error(USAGE);
  return {
    workflow: resolve(workflow),
    target: resolve(target),
    ...(runtime === undefined ? {} : { runtime }),
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

/**
 * The command (`bin/codemod-workflow.mjs`): run `argv`, print the result as
 * JSON on stdout or the error on stderr (exit code 1), with
 * SIGINT/SIGTERM/SIGHUP aborting the run.
 */
export async function main(argv: string[]): Promise<void> {
  // Benchmark timings (no-ops when off): `boot` ends here; printing the
  // result is `plan` (the run itself times its own phases).
  endBoot();
  let printed = 0;
  const controller = new AbortController();
  const abort = () => controller.abort();
  for (const name of ABORT_SIGNALS) process.once(name, abort);
  try {
    const output = await runWorkflowCli(argv, controller.signal);
    printed = now();
    process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  } finally {
    for (const name of ABORT_SIGNALS) process.off(name, abort);
    if (printed !== 0) lap(Phase.plan, printed);
  }
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  void main(process.argv.slice(2));
}

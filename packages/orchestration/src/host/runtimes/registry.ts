/**
 * The runtime registry and the one launch path both the CLI and the
 * programmatic API use. Runtimes are looked up by name; the built-in list is
 * `defaultRuntimes` (one provider per file in this folder).
 */
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import type { Json } from "../../core/json.ts";
import { DEFAULT_RUNTIME } from "../../runtime/capabilities.ts";
import { Phase, lap, now } from "../../core/timings.ts";
import { quickjsRuntime } from "./quickjs.ts";
import type { RuntimeProvider, RuntimeSettings } from "./types.ts";

export class UnknownRuntimeError extends Error {
  constructor(
    readonly runtime: string,
    known: readonly string[],
  ) {
    super(`unknown runtime '${runtime}'; expected one of: ${known.join(", ")}`);
    this.name = "UnknownRuntimeError";
  }
}

/** A runtime that exists but cannot run here (not built, not installed, not implemented yet). */
export class RuntimeUnavailableError extends Error {
  constructor(
    readonly runtime: string,
    reason: string,
  ) {
    super(reason);
    this.name = "RuntimeUnavailableError";
  }
}

export class RuntimeRegistry {
  readonly #providers = new Map<string, RuntimeProvider>();

  constructor(providers: Iterable<RuntimeProvider> = []) {
    for (const provider of providers) this.register(provider);
  }

  /** Add or replace (by name). */
  register(provider: RuntimeProvider): this {
    this.#providers.set(provider.name, provider);
    return this;
  }

  get(name: string): RuntimeProvider | undefined {
    return this.#providers.get(name);
  }

  names(): string[] {
    return [...this.#providers.keys()];
  }

  /** The provider, or `UnknownRuntimeError`. */
  resolve(name: string): RuntimeProvider {
    const provider = this.#providers.get(name);
    if (provider === undefined) throw new UnknownRuntimeError(name, this.names());
    return provider;
  }
}

export const defaultRuntimes = new RuntimeRegistry([quickjsRuntime]);

/** Throws `RuntimeUnavailableError` with the provider's reason. */
export function assertAvailable(provider: RuntimeProvider, settings: RuntimeSettings): void {
  const availability = provider.availability(settings);
  if (!availability.available) {
    throw new RuntimeUnavailableError(provider.name, availability.reason);
  }
}

export interface RunWorkflowOptions {
  /** Runtime name. Default `quickjs`. */
  runtime?: string;
  /** Repository the workflow operates on. Default: the process working directory. */
  target?: string;
  /** Extra environment for spawned processes. */
  env?: Record<string, string>;
  /** The root input; the key's presence matters (`null` is a value). */
  input?: Json;
  signal?: AbortSignal;
  /** Out-of-band lines (stderr in the CLI). */
  notify?: (line: string) => void;
  /** Default `defaultRuntimes`. */
  registry?: RuntimeRegistry;
}

export interface PreparedLaunch {
  provider: RuntimeProvider;
  /** Absolute workflow module path. */
  workflow: string;
  /** Absolute repository root. */
  target: string;
  settings: RuntimeSettings;
}

/**
 * Resolve the runtime by name, check the workflow and target exist, and check
 * the runtime is available, in that order; nothing is loaded yet.
 */
export function prepareLaunch(
  workflowPath: string,
  options: Omit<RunWorkflowOptions, "input" | "signal" | "notify">,
): PreparedLaunch {
  const registry = options.registry ?? defaultRuntimes;
  const provider = registry.resolve(options.runtime ?? DEFAULT_RUNTIME);
  const workflow = resolve(workflowPath);
  const target = resolve(options.target ?? process.cwd());
  if (!existsSync(workflow)) throw new Error(`workflow does not exist: ${workflow}`);
  if (!existsSync(target)) throw new Error(`target does not exist: ${target}`);
  const settings: RuntimeSettings = options.env === undefined ? {} : { env: options.env };
  assertAvailable(provider, settings);
  return { provider, workflow, target, settings };
}

/**
 * Run a workflow file on a named runtime and resolve with its final output:
 * the programmatic form of `codemod-workflow run --runtime=<name>`.
 */
export async function runWorkflow(
  workflowPath: string,
  options: RunWorkflowOptions = {},
): Promise<unknown> {
  // Benchmark timings (no-ops when off): resolving the runtime is `plan`
  // (the provider times finding or building its bundle itself).
  const planned = now();
  const { provider, workflow, target, settings } = prepareLaunch(workflowPath, options);
  lap(Phase.plan, planned);
  return provider.launch({
    workflow,
    target,
    settings,
    ...("input" in options ? { input: options.input } : {}),
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    ...(options.notify === undefined ? {} : { notify: options.notify }),
  });
}

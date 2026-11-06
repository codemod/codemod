/**
 * The runtime registry and the one launch path both the CLI and the
 * programmatic API use. Runtimes are looked up by name; the built-in list is
 * `defaultRuntimes` (one provider per file in this folder).
 */
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { loadWorkflow } from "../../bundle/build.ts";
import {
  executableRequiresInput,
  isExecutable,
  type Executable,
} from "../../authoring/composition.ts";
import type { ArtifactStore } from "../../bundle/build.ts";
import type { Json } from "../../core/json.ts";
import { DEFAULT_RUNTIME } from "../../runtime/capabilities.ts";
import { run } from "../../runtime/run.ts";
import "../node-host.ts";
import { bridgeRuntime, defaultBridgePath } from "./bridge.ts";
import { nodeRuntime } from "./node.ts";
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

  /**
   * Add or replace (by name). A provider with neither `createExecutor` nor
   * `launch` is a placeholder whose `availability` explains why.
   */
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

export const defaultRuntimes = new RuntimeRegistry([bridgeRuntime, nodeRuntime, quickjsRuntime]);

/** Throws `RuntimeUnavailableError` with the provider's reason. */
export function assertAvailable(provider: RuntimeProvider, settings: RuntimeSettings): void {
  const availability = provider.availability(settings);
  if (!availability.available) {
    throw new RuntimeUnavailableError(provider.name, availability.reason);
  }
}

export interface RunWorkflowOptions {
  /** Runtime name. Default `bridge`. */
  runtime?: string;
  /** Repository the workflow operates on. Default: the process working directory. */
  target?: string;
  /** Bridge binary. Default `defaultBridgePath()`. */
  bridge?: string;
  /** Extra environment for spawned processes. */
  env?: Record<string, string>;
  /** The root input; the key's presence matters (`null` is a value). */
  input?: Json;
  signal?: AbortSignal;
  /** Out-of-band lines (out-of-process runtimes). */
  notify?: (line: string) => void;
  /** Default `defaultRuntimes`. */
  registry?: RuntimeRegistry;
}

/** A loaded, validated workflow module ready for an in-process run. */
export interface LoadedExecutable {
  executable: Executable;
  artifacts: ArtifactStore;
}

/**
 * Load a workflow module through the build step and validate its default
 * export, and that an input is given when the root needs one.
 */
export async function loadExecutable(
  workflow: string,
  hasInput: boolean,
): Promise<LoadedExecutable> {
  const { exports, artifacts } = await loadWorkflow(workflow);
  const executable = exports.default;
  if (!isExecutable(executable)) {
    throw new Error(
      "workflow module must default-export a shell or jssg step, a step invocation, dynamic(...), sequence(...), or parallel(...)",
    );
  }
  if (!hasInput && executableRequiresInput(executable)) {
    throw new Error("workflow requires an input value; pass --input <json>");
  }
  return { executable, artifacts };
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
  const settings: RuntimeSettings = {
    bridge: resolve(options.bridge ?? defaultBridgePath()),
    ...(options.env === undefined ? {} : { env: options.env }),
  };
  assertAvailable(provider, settings);
  return { provider, workflow, target, settings };
}

/**
 * Run a workflow file on a named runtime and resolve with its final output:
 * the programmatic form of `codemod-workflow run --runtime=<name>`.
 * In-process runtimes load the module here and run it with the provider's
 * executor and capability contract; out-of-process runtimes `launch` it.
 */
export async function runWorkflow(
  workflowPath: string,
  options: RunWorkflowOptions = {},
): Promise<unknown> {
  const { provider, workflow, target, settings } = prepareLaunch(workflowPath, options);
  const hasInput = "input" in options;
  if (provider.createExecutor === undefined) {
    if (provider.launch === undefined) {
      throw new RuntimeUnavailableError(
        provider.name,
        `runtime '${provider.name}' provides neither an executor nor a launcher`,
      );
    }
    return provider.launch({
      workflow,
      target,
      settings,
      ...(hasInput ? { input: options.input } : {}),
      ...(options.signal === undefined ? {} : { signal: options.signal }),
      ...(options.notify === undefined ? {} : { notify: options.notify }),
    });
  }
  const { executable, artifacts } = await loadExecutable(workflow, hasInput);
  const result = await run(executable, {
    executor: provider.createExecutor({ cwd: target, artifacts, settings }),
    runtime: provider,
    signal: options.signal,
    ...(hasInput ? { input: options.input } : {}),
  });
  return result.output;
}

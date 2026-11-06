/**
 * The runtime provider contract: what `codemod-workflow run --runtime=<name>`
 * and `runWorkflow(path, { runtime })` need from a runtime. One provider per
 * file in this folder (`bridge.ts`, `node.ts`, `quickjs.ts`); `registry.ts`
 * lists them. Adding or replacing a runtime never touches the CLI.
 *
 * A provider is one of two shapes:
 *
 * - in-process (`createExecutor`): the control plane (`run()`, replay,
 *   history, scheduler) runs in this Node process over the loaded workflow
 *   module; the provider only decides how operations execute. `bridge` and
 *   `node`.
 * - out-of-process (`launch`): the provider runs the whole workflow file
 *   elsewhere and returns its final output. `quickjs` (build the bundle, run
 *   it in the Rust QuickJS host binary).
 *
 * Either way the capability contract (`runtime/capabilities.ts`) applies:
 * in-process runs pass the provider as `RunSettings.runtime`; an
 * out-of-process host must pass the same descriptor to `run()` inside its
 * own control plane.
 */
import type { ArtifactStore } from "../../bundle/build.ts";
import type { EventSink } from "../../core/events.ts";
import type { Json } from "../../core/json.ts";
import type { OperationExecutor } from "../../execution/operation-executor.ts";
import type { RuntimeDescriptor } from "../../runtime/capabilities.ts";

/** Host configuration shared by every runtime; never part of history. */
export interface RuntimeSettings {
  /** `butterflow-execution-bridge` binary (the `bridge` runtime; others may ignore it). */
  bridge: string;
  /** Extra environment for spawned processes, as `BridgeOptions.env`. */
  env?: Record<string, string>;
}

export type RuntimeAvailability = { available: true } | { available: false; reason: string };

/** Everything an in-process runtime gets to build the executor for one run. */
export interface ExecutorContext {
  /** Absolute repository root: `shell` cwd and JSSG target root. */
  cwd: string;
  /** Built transform artifacts by hash (`loadWorkflow()`). */
  artifacts: ArtifactStore;
  /** The run's event sink. */
  events?: EventSink;
  settings: RuntimeSettings;
}

/** One out-of-process launch of a workflow file. */
export interface LaunchRequest {
  /** Absolute path of the workflow module. */
  workflow: string;
  /** Absolute repository root. */
  target: string;
  /** Present exactly when an input was given (`null` is a value). */
  input?: Json;
  signal?: AbortSignal;
  settings: RuntimeSettings;
  /** Out-of-band progress lines (stderr in the CLI). */
  notify?: (line: string) => void;
}

export interface RuntimeProvider extends RuntimeDescriptor {
  /** One line for `--help` / errors. */
  readonly description: string;
  /**
   * Whether this runtime can run here with these settings. Checked before the
   * workflow is loaded; `reason` is the whole user-facing error.
   */
  availability(settings: RuntimeSettings): RuntimeAvailability;
  /**
   * Import the implementation. `runWorkflow` and the CLI await it once the
   * runtime is available, before `createExecutor` or `launch`, so resolving
   * and checking a runtime (and running a different one) loads none of its
   * code. Idempotent.
   */
  load?(): Promise<void>;
  /** In-process runtimes: the executor for one run (called once per run, after `load`). */
  createExecutor?(context: ExecutorContext): OperationExecutor;
  /** Out-of-process runtimes: run the workflow file end to end; resolves with its output. */
  launch?(request: LaunchRequest): Promise<unknown>;
}

/** `runtime '<name>' not available yet`: what placeholder providers report. */
export function notAvailableYet(name: string): RuntimeAvailability {
  return { available: false, reason: `runtime '${name}' not available yet` };
}

/** What a provider's `createExecutor` throws when called before its `load()` resolved. */
export function notLoaded(name: string): Error {
  return new Error(`runtime '${name}' is not loaded; await its load() before createExecutor()`);
}

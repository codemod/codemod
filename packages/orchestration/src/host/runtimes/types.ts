/**
 * The runtime provider contract: what `codemod-workflow run --runtime=<name>`
 * and `runWorkflow(path, { runtime })` need from a runtime. One provider per
 * file in this folder (`quickjs.ts`); `registry.ts` lists them. Adding or
 * replacing a runtime never touches the CLI.
 *
 * A provider runs the whole workflow file out of process (`launch`) and
 * returns its final output; `quickjs` builds the bundle and runs it in the
 * Rust QuickJS host binary. The host passes the provider's capability
 * descriptor (`runtime/capabilities.ts`) to `run()` inside its own control
 * plane.
 */
import type { Json } from "../../core/json.ts";
import type { RuntimeDescriptor } from "../../runtime/capabilities.ts";

/** Host configuration shared by every runtime; never part of history. */
export interface RuntimeSettings {
  /** Extra environment for spawned processes (`shell` steps, agent runners). */
  env?: Record<string, string>;
}

export type RuntimeAvailability = { available: true } | { available: false; reason: string };

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
  /** Run the workflow file end to end; resolves with its output. */
  launch(request: LaunchRequest): Promise<unknown>;
}

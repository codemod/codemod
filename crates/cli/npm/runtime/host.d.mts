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
 * so they can be debugged, covered and mocked. Host tasks (`jssg`, `shell`,
 * `codemod`, ...) and the run store go through the native addon.
 *
 * Code here has whatever Node.js gives it: the capabilities codemod.yaml
 * declares are not enforced. The codemod CLI runs workflows in its own
 * sandboxed runtime instead; run packages from the registry with the CLI.
 *
 * @module
 */

import type { JsonValue } from "../testing/index.d.mts";

/** A `URL` (`new URL("./workflow.ts", import.meta.url)`); only `file:` URLs are paths. */
export interface UrlLike {
  readonly href: string;
}

/** An `AbortSignal`. */
export interface AbortSignalLike {
  readonly aborted: boolean;
  addEventListener(type: "abort", listener: () => void, options?: { once?: boolean }): void;
  removeEventListener(type: "abort", listener: () => void): void;
}

/** A workflow module: what `import("./workflow.ts")` gives. */
export interface WorkflowModule {
  default: (context: {
    params: Record<string, JsonValue>;
    target: string;
    dryRun: boolean;
  }) => unknown;
}

export interface HostRunOptions {
  /** The directory the workflow changes. Default: the current directory. */
  target?: string;
  /** The workflow's parameters. */
  params?: Record<string, JsonValue>;
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
  readonly status: "succeeded" | "failed";
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
  readonly metrics: Record<string, unknown>;
  readonly stats: {
    readonly filesModified: number;
    readonly filesUnmodified: number;
    readonly filesWithErrors: number;
  };
  /** What the engine reported while the run progressed. */
  readonly events: readonly ({ readonly event: string } & Record<string, unknown>)[];
}

/**
 * Run a workflow (a workflow file, a package directory, or the workflow
 * module itself). Resolves for every outcome; rejects when the workflow
 * cannot be loaded. YAML workflows run only with the CLI.
 */
export function runWorkflow(
  workflow: string | UrlLike | WorkflowModule,
  options?: HostRunOptions,
): Promise<HostRunReport>;

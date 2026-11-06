/**
 * The runtime capability contract, portable so it is enforced by the control
 * plane wherever it runs (in Node for `bridge`/`node`, inside QuickJS for
 * `quickjs`).
 *
 * A runtime is where workflow code and its operations execute:
 *
 * - `bridge` (default): the control plane in Node; every `shell`, `jssg`,
 *   and `agent` operation in one `butterflow-execution-bridge` process;
 *   `assessment` from the Node host.
 * - `node`: the whole workflow in one Node process; JSSG transforms in-process
 *   over a napi-rs ast-grep addon; `shell` through `child_process`.
 * - `quickjs`: the whole prebuilt workflow bundle in one QuickJS context
 *   hosted by a Rust binary; no Node at runtime.
 *
 * `node` and `quickjs` execute `jssg` and `shell` only; `agent` and
 * `assessment` fail fast, and so does JSSG semantic analysis (deferred). The
 * refusal happens twice: statically, before the run starts, for every
 * operation the static topology names (`assertRuntimeSupports`), and at the
 * execution boundary for operations issued inside `dynamic()`
 * (`CapabilityExecutor`), where it is a `failed` completion with
 * `details.phase: "config"` and nothing executed.
 */
import { isRunnable, type OperationKind } from "../authoring/runnable.ts";
import { executableIr, type CompositionIrNode, type Executable } from "../authoring/composition.ts";
import type { OperationExecutor } from "../execution/operation-executor.ts";
import {
  PROTOCOL_VERSION,
  type Operation,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";

/** The runtimes `codemod-workflow run --runtime` knows by name. */
export const RUNTIME_NAMES = ["bridge", "node", "quickjs"] as const;
export type BuiltinRuntimeName = (typeof RUNTIME_NAMES)[number];
export const DEFAULT_RUNTIME: BuiltinRuntimeName = "bridge";

export interface RuntimeCapabilities {
  /** Operation kinds the runtime executes. */
  readonly operations: Readonly<Record<OperationKind, boolean>>;
  /** JSSG `semanticAnalysis` (definition/references providers). */
  readonly semanticAnalysis: boolean;
}

/** Name plus capabilities: what the control plane needs to know about a runtime. */
export interface RuntimeDescriptor {
  readonly name: string;
  readonly capabilities: RuntimeCapabilities;
}

const ALL: RuntimeCapabilities = {
  operations: { shell: true, jssg: true, agent: true, assessment: true },
  semanticAnalysis: true,
};

const JSSG_AND_SHELL: RuntimeCapabilities = {
  operations: { shell: true, jssg: true, agent: false, assessment: false },
  semanticAnalysis: false,
};

export const BRIDGE_RUNTIME: RuntimeDescriptor = { name: "bridge", capabilities: ALL };
export const NODE_RUNTIME: RuntimeDescriptor = { name: "node", capabilities: JSSG_AND_SHELL };
export const QUICKJS_RUNTIME: RuntimeDescriptor = {
  name: "quickjs",
  capabilities: JSSG_AND_SHELL,
};

export const BUILTIN_RUNTIMES: Readonly<Record<BuiltinRuntimeName, RuntimeDescriptor>> = {
  bridge: BRIDGE_RUNTIME,
  node: NODE_RUNTIME,
  quickjs: QUICKJS_RUNTIME,
};

/** A workflow uses something its runtime cannot execute. */
export class UnsupportedInRuntimeError extends Error {
  constructor(
    readonly runtime: string,
    readonly feature: string,
    readonly commandId?: string,
  ) {
    super(
      `${feature} is unsupported in runtime '${runtime}'${commandId === undefined ? "" : ` (command '${commandId}')`}`,
    );
    this.name = "UnsupportedInRuntimeError";
  }
}

/** Why `runtime` cannot execute `operation`, or `undefined` when it can. */
export function unsupportedFeature(
  capabilities: RuntimeCapabilities,
  operation: Operation,
): string | undefined {
  if (!capabilities.operations[operation.kind]) return `operation kind '${operation.kind}'`;
  if (
    operation.kind === "jssg" &&
    operation.semanticAnalysis !== undefined &&
    !capabilities.semanticAnalysis
  ) {
    return "JSSG semantic analysis";
  }
  return undefined;
}

/**
 * Static preflight: every operation node the static topology names must be
 * executable. Operations inside `dynamic()` are opaque here and are refused
 * by `CapabilityExecutor` when issued. JSSG semantic analysis is invisible in
 * the IR, so a bare runnable root is checked through its operation when it
 * needs no input; otherwise semantic analysis is refused at execution.
 */
export function assertRuntimeSupports(runtime: RuntimeDescriptor, executable: Executable): void {
  const visit = (node: CompositionIrNode): void => {
    if (node.type === "operation") {
      if (!(runtime.capabilities.operations as Record<string, boolean>)[node.kind]) {
        throw new UnsupportedInRuntimeError(runtime.name, `operation kind '${node.kind}'`, node.id);
      }
      return;
    }
    if (node.type === "dynamic") return;
    for (const child of node.type === "sequence" ? node.stages : node.members) visit(child);
  };
  visit(executableIr(executable));
  if (isRunnable(executable) && executable.kind === "jssg" && executable.input === undefined) {
    const feature = unsupportedFeature(runtime.capabilities, executable.toOperation(undefined));
    if (feature !== undefined) {
      throw new UnsupportedInRuntimeError(runtime.name, feature, executable.name);
    }
  }
}

/**
 * The execution-boundary guard: refuses what the runtime cannot execute with
 * a `failed` completion (`details.phase: "config"`, nothing executed, the
 * repository untouched) and delegates everything else.
 */
export class CapabilityExecutor implements OperationExecutor {
  constructor(
    readonly runtime: RuntimeDescriptor,
    private readonly inner: OperationExecutor,
  ) {}

  execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    const feature = unsupportedFeature(this.runtime.capabilities, request.operation);
    if (feature === undefined) return this.inner.execute(request, signal);
    return Promise.resolve({
      protocolVersion: PROTOCOL_VERSION,
      commandId: request.commandId,
      status: "failed",
      error: {
        message: new UnsupportedInRuntimeError(this.runtime.name, feature).message,
        details: {
          phase: "config",
          runtime: this.runtime.name,
          unsupported: feature,
          repositoryMayBeModified: false,
        },
      },
    });
  }
}

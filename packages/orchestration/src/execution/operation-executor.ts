/**
 * Migration seam: how an `OperationRequest` becomes an `OperationCompletion`.
 * Implementations: `BridgeExecutor` (the Rust bridge, `executor.ts`), the
 * runtime executors registered in `runtime/registry.ts`, and the harness's
 * scripted executor. Portable: no host access.
 */
import type { OperationCompletion, OperationRequest } from "../core/protocol.ts";

export interface OperationExecutor {
  /** `signal` aborts the operation; the completion is then `cancelled` or `unknown`. */
  execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion>;
}

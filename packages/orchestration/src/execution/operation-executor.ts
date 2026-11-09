/**
 * Migration seam: how an `OperationRequest` becomes an `OperationCompletion`.
 * Implementations: the QuickJS bundle's executor and the harness's scripted
 * executor. Portable: no host access.
 */
import type { OperationCompletion, OperationRequest } from "../core/protocol.ts";

export interface OperationExecutor {
  /** `signal` aborts the operation; the completion is then `cancelled` or `unknown`. */
  execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion>;
}

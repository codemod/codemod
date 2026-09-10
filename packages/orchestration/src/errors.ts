import type { CompletionError, CompletionStatus } from "./protocol.ts";

/** Thrown from an awaited command when it did not succeed. */
export class OperationError extends Error {
  constructor(
    readonly commandId: string,
    readonly status: Exclude<CompletionStatus, "succeeded">,
    readonly detail: CompletionError | undefined,
  ) {
    super(`command '${commandId}' ${status}${detail ? `: ${detail.message}` : ""}`);
    this.name = "OperationError";
  }
}

export type NondeterminismKind =
  /** Same command id, different command content. */
  | "changed"
  /** A recorded command was issued later than its recorded position. */
  | "reordered"
  /** A new command was issued after the workflow had already finalized. */
  | "added"
  /** Recorded commands were never issued again (middle or trailing). */
  | "removed"
  /** The workflow returned a different final output than the recorded one. */
  | "output";

/** Replay found that the workflow code no longer matches its history. */
export class NondeterminismError extends Error {
  constructor(
    readonly kind: NondeterminismKind,
    message: string,
    readonly detail: {
      position?: number;
      expectedId?: string;
      actualId?: string;
      missing?: string[];
    } = {},
  ) {
    super(`nondeterministic workflow (${kind}): ${message}`);
    this.name = "NondeterminismError";
  }
}

/** Two commands in one run resolved to the same command id. */
export class DuplicateCommandIdError extends Error {
  constructor(readonly commandId: string) {
    super(
      `command id '${commandId}' was issued twice in one run; pass an explicit { id } when invoking the same runnable repeatedly`,
    );
    this.name = "DuplicateCommandIdError";
  }
}

/** The options passed when invoking a runnable are malformed. */
export class InvocationError extends Error {
  constructor(
    readonly where: string,
    message: string,
  ) {
    super(`invalid invocation of ${where}: ${message}`);
    this.name = "InvocationError";
  }
}

/**
 * A JSSG invocation target is malformed, or a target was handed to a runnable
 * that cannot enforce it (`exec` or `ai`).
 */
export class TargetValidationError extends Error {
  constructor(
    readonly where: string,
    message: string,
  ) {
    super(`invalid target for ${where}: ${message}`);
    this.name = "TargetValidationError";
  }
}

export class PlanValidationError extends Error {
  constructor(message: string) {
    super(`invalid plan: ${message}`);
    this.name = "PlanValidationError";
  }
}

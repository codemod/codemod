import type { ScheduledCommand } from "./history.ts";
import type { Json } from "./json.ts";
import type { OperationCompletion } from "./protocol.ts";

/** Phases of one JSSG command; also the `phase` in a JSSG failure's details. */
export type JssgPhase = "open" | "select" | "index" | "transform" | "stage" | "commit";

export type WorkflowEvent =
  | { type: "command.scheduled"; command: ScheduledCommand }
  | { type: "command.replayed"; commandId: string; completion: OperationCompletion }
  | { type: "command.completed"; commandId: string; completion: OperationCompletion }
  | { type: "workflow.finished"; output: Json; replayed: boolean }
  /** One Rust worker was spawned for a JSSG command. */
  | { type: "jssg.worker"; commandId: string; pid: number | undefined }
  | {
      type: "jssg.progress";
      commandId: string;
      phase: JssgPhase;
      path?: string;
      completed: number;
      total: number;
      skipped?: string;
    };

/** Migration seam: where engine events go (CLI, TUI, JSONL, ...). */
export interface EventSink {
  emit(event: WorkflowEvent): void;
}

export const nullSink: EventSink = { emit() {} };

export class CollectingSink implements EventSink {
  readonly events: WorkflowEvent[] = [];
  emit(event: WorkflowEvent): void {
    this.events.push(event);
  }
}

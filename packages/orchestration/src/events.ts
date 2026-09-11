import type { ScheduledCommand } from "./history.ts";
import type { Json } from "./json.ts";
import type { OperationCompletion } from "./protocol.ts";

export type WorkflowEvent =
  | { type: "command.scheduled"; command: ScheduledCommand }
  | { type: "command.replayed"; commandId: string; completion: OperationCompletion }
  | { type: "command.completed"; commandId: string; completion: OperationCompletion }
  | { type: "workflow.finished"; output: Json; replayed: boolean }
  /** One bridge process was spawned for a command. */
  | { type: "bridge.spawned"; commandId: string; pid: number | undefined };

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

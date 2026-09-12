/**
 * Append-only history. Everything here is plain JSON so a store can be
 * reimplemented in Rust (SQLite, files) without touching workflow source.
 */
import { cloneJson, type Json } from "./json.ts";
import { PROTOCOL_VERSION, type Operation, type OperationCompletion } from "./protocol.ts";

/** The canonical JSON of this record is the command identity used for replay. */
export interface ScheduledCommand {
  id: string;
  runnable: string;
  kind: Operation["kind"];
  input?: Json;
  operation: Operation;
}

export type HistoryEvent =
  | { type: "scheduled"; command: ScheduledCommand }
  | { type: "completed"; commandId: string; completion: OperationCompletion }
  | { type: "finalized"; output: Json };

export interface History {
  protocolVersion: typeof PROTOCOL_VERSION;
  events: HistoryEvent[];
}

export function emptyHistory(): History {
  return { protocolVersion: PROTOCOL_VERSION, events: [] };
}

/** Migration seam: append-only persistence. */
export interface HistoryStore {
  load(): Promise<History>;
  append(event: HistoryEvent): Promise<void>;
}

export class MemoryHistoryStore implements HistoryStore {
  private history: History;

  constructor(history: History = emptyHistory()) {
    if (history.protocolVersion !== PROTOCOL_VERSION) {
      throw new Error(`unsupported history protocolVersion ${String(history.protocolVersion)}`);
    }
    this.history = cloneJson(history);
  }

  static fromJSON(text: string): MemoryHistoryStore {
    return new MemoryHistoryStore(JSON.parse(text) as History);
  }

  async load(): Promise<History> {
    return cloneJson(this.history);
  }

  async append(event: HistoryEvent): Promise<void> {
    this.history.events.push(cloneJson(event));
  }

  toJSON(): History {
    return cloneJson(this.history);
  }

  serialize(): string {
    return JSON.stringify(this.history);
  }
}

export function scheduledCommands(history: History): ScheduledCommand[] {
  return history.events.flatMap((event) => (event.type === "scheduled" ? [event.command] : []));
}

export function completions(history: History): Map<string, OperationCompletion> {
  const map = new Map<string, OperationCompletion>();
  for (const event of history.events) {
    if (event.type === "completed") map.set(event.commandId, event.completion);
  }
  return map;
}

export function finalOutput(
  history: History,
): { finalized: true; output: Json } | { finalized: false } {
  for (const event of history.events) {
    if (event.type === "finalized") return { finalized: true, output: event.output };
  }
  return { finalized: false };
}

/**
 * CommandGate: the replay/execute decision point. Every issued command becomes
 * a `resolve`, and the workflow's return becomes a `finish`. The gate is the
 * only place that reads or appends history.
 */
import { NondeterminismError, DuplicateCommandIdError } from "./errors.ts";
import { nullSink, type EventSink } from "./events.ts";
import type { OperationExecutor } from "./executor.ts";
import {
  completions,
  finalOutput,
  scheduledCommands,
  type History,
  type HistoryStore,
  type ScheduledCommand,
} from "./history.ts";
import { canonicalJson, type Json } from "./json.ts";
import { PROTOCOL_VERSION, type OperationCompletion } from "./protocol.ts";

/** Migration seam: resolve a command (replay or execute) and finish a run. */
export interface CommandGate {
  resolve(command: ScheduledCommand): Promise<OperationCompletion>;
  finish(output: Json): Promise<{ replayed: boolean }>;
}

export class ReplayGate implements CommandGate {
  private readonly recorded: ScheduledCommand[];
  private readonly recordedIndex = new Map<string, number>();
  private readonly recordedCompletions: Map<string, OperationCompletion>;
  private readonly recordedFinal: ReturnType<typeof finalOutput>;
  private readonly issued = new Set<string>();
  /** Recorded positions that were skipped over by a later recorded command. */
  private readonly skipped = new Map<number, string>();
  private cursor = 0;

  constructor(
    history: History,
    private readonly store: HistoryStore,
    private readonly executor: OperationExecutor,
    private readonly events: EventSink = nullSink,
    private readonly signal?: AbortSignal,
  ) {
    this.recorded = scheduledCommands(history);
    this.recorded.forEach((command, index) => this.recordedIndex.set(command.id, index));
    this.recordedCompletions = completions(history);
    this.recordedFinal = finalOutput(history);
  }

  async resolve(command: ScheduledCommand): Promise<OperationCompletion> {
    if (this.issued.has(command.id)) throw new DuplicateCommandIdError(command.id);
    this.issued.add(command.id);

    const position = this.recordedIndex.get(command.id);
    if (position === undefined) return this.executeNew(command);

    const recorded = this.recorded[position]!;
    if (canonicalJson(recorded) !== canonicalJson(command)) {
      throw new NondeterminismError(
        "changed",
        `command '${command.id}' differs from its recorded version at position ${position}`,
        { position, expectedId: recorded.id, actualId: command.id },
      );
    }
    if (this.skipped.has(position)) {
      throw new NondeterminismError(
        "reordered",
        `command '${command.id}' was recorded at position ${position} but issued after later commands`,
        { position, actualId: command.id },
      );
    }
    for (let index = this.cursor; index < position; index++) {
      this.skipped.set(index, this.recorded[index]!.id);
    }
    this.cursor = Math.max(this.cursor, position + 1);

    const completion = this.recordedCompletions.get(command.id) ?? {
      protocolVersion: PROTOCOL_VERSION,
      commandId: command.id,
      status: "unknown",
      error: { message: "command was scheduled in history but never completed" },
    };
    this.events.emit({ type: "command.replayed", commandId: command.id, completion });
    return completion;
  }

  private async executeNew(command: ScheduledCommand): Promise<OperationCompletion> {
    if (this.recordedFinal.finalized) {
      throw new NondeterminismError(
        "added",
        `command '${command.id}' was issued after the workflow had finalized`,
        {
          actualId: command.id,
        },
      );
    }
    if (this.skipped.size > 0) {
      throw new NondeterminismError(
        "removed",
        `recorded commands were skipped before new command '${command.id}': ${[...this.skipped.values()].join(", ")}`,
        { actualId: command.id, missing: [...this.skipped.values()] },
      );
    }
    if (this.cursor < this.recorded.length) {
      const expected = this.recorded[this.cursor]!;
      throw new NondeterminismError(
        "changed",
        `expected recorded command '${expected.id}' at position ${this.cursor} but got new command '${command.id}'`,
        { position: this.cursor, expectedId: expected.id, actualId: command.id },
      );
    }

    await this.store.append({ type: "scheduled", command });
    this.events.emit({ type: "command.scheduled", command });
    const completion = await this.executor.execute(
      {
        protocolVersion: PROTOCOL_VERSION,
        commandId: command.id,
        operation: command.operation,
      },
      this.signal,
    );
    if (completion.commandId !== command.id) {
      throw new Error(
        `executor returned completion for '${completion.commandId}' while running '${command.id}'`,
      );
    }
    await this.store.append({ type: "completed", commandId: command.id, completion });
    this.events.emit({ type: "command.completed", commandId: command.id, completion });
    return completion;
  }

  async finish(output: Json): Promise<{ replayed: boolean }> {
    if (this.skipped.size > 0) {
      const missing = [...this.skipped.values()];
      throw new NondeterminismError(
        "removed",
        `recorded commands were never issued: ${missing.join(", ")}`,
        {
          missing,
        },
      );
    }
    if (this.cursor < this.recorded.length) {
      const missing = this.recorded.slice(this.cursor).map((command) => command.id);
      throw new NondeterminismError(
        "removed",
        `workflow finished before issuing recorded trailing commands: ${missing.join(", ")}`,
        { position: this.cursor, missing },
      );
    }
    if (this.recordedFinal.finalized) {
      if (canonicalJson(this.recordedFinal.output) !== canonicalJson(output)) {
        throw new NondeterminismError(
          "output",
          "workflow returned a different final output than recorded",
        );
      }
      this.events.emit({ type: "workflow.finished", output, replayed: true });
      return { replayed: true };
    }
    await this.store.append({ type: "finalized", output });
    this.events.emit({ type: "workflow.finished", output, replayed: false });
    return { replayed: false };
  }
}

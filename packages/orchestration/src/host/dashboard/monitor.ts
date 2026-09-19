/**
 * Dashboard-facing view of one run.
 *
 * `RunMonitor` is an `EventSink`. Every `RunEvent` is projected to a payload
 * that carries no operation content (no `env`, no bound inputs, no outputs),
 * stamped with the run id, a monotonic sequence number, and a timestamp,
 * folded into an in-memory snapshot, retained in a bounded buffer for
 * reconnect catch-up, and handed to subscribers. `emit` is synchronous
 * bookkeeping that never waits on a subscriber, so a slow consumer cannot
 * hold up the run; the HTTP layer keeps its own writes non-blocking.
 *
 * None of this is history. Command identity and replay (`core/history.ts`)
 * are untouched, and the run id exists only for this process.
 */
import { randomUUID } from "node:crypto";
import type { CompositionIrNode, OperationIr } from "../../authoring/composition.ts";
import type { EventSink, RunEvent } from "../../core/events.ts";
import type { CompletionStatus, Target } from "../../core/protocol.ts";

/**
 * `running` and `paused` describe admission while the run is open: paused
 * means nothing more is admitted, not that nothing is executing. The terminal
 * states come from `run()` resolving (`completed`), rejecting (`failed`), or
 * rejecting after the host's abort signal fired (`cancelled`).
 */
export type RunStatus = "running" | "paused" | "completed" | "failed" | "cancelled";

/**
 * `scheduled` (recorded, admission not decided) -> `queued` | `running` -> a
 * completion status. `replayed` commands come straight from history and carry
 * their recorded status in `CommandView.status`.
 */
export type CommandState = "scheduled" | "queued" | "running" | "replayed" | CompletionStatus;

export interface CommandView {
  id: string;
  runnable?: string;
  kind?: string;
  state: CommandState;
  /** Named by the static topology; otherwise issued at runtime inside `dynamic()`. */
  static: boolean;
  /** Static-topology path of the dynamic node that issued this command. */
  dynamicId?: string;
  concurrent?: true;
  target?: Target;
  weight?: number;
  pid?: number;
  /** The recorded completion status of a replayed command. */
  status?: CompletionStatus;
  error?: string;
  /** JSSG failure phase when the completion reported one. */
  phase?: string;
  scheduledAt: string;
  startedAt?: string;
  finishedAt?: string;
}

export interface SchedulerView {
  /** Unknown until the scheduler admits something or the host supplies it. */
  capacity?: number;
  used: number;
  active: number;
  queued: number;
  paused: boolean;
}

export interface DashboardSnapshot {
  runId: string;
  workflow: string;
  startedAt: string;
  finishedAt?: string;
  status: RunStatus;
  /** Sequence number of the last envelope folded in; catch-up starts after it. */
  seq: number;
  topology: CompositionIrNode;
  scheduler: SchedulerView;
  /** In first-seen order. */
  commands: CommandView[];
  /** Why a failed or cancelled run ended. */
  error?: string;
  replayed?: boolean;
}

/** `RunEvent` without operation content, plus the two outcomes only the host observes. */
export type DashboardEvent =
  | {
      type: "command.scheduled";
      commandId: string;
      runnable: string;
      kind: string;
      concurrent?: true;
      dynamicId?: string;
      target?: Target;
    }
  | {
      type: "command.replayed";
      commandId: string;
      status: CompletionStatus;
      dynamicId?: string;
    }
  | {
      type: "command.completed";
      commandId: string;
      status: CompletionStatus;
      error?: string;
      phase?: string;
    }
  | { type: "run.finished"; replayed: boolean }
  | { type: "run.failed"; message: string }
  | { type: "run.cancelled"; message: string }
  | Extract<RunEvent, { type: `scheduler.${string}` | "bridge.spawned" }>;

/**
 * One stream entry. Besides the event it carries the run and scheduler state
 * after the event and the command it changed, so a client applies it without
 * its own reducer and a snapshot plus every later envelope is always exact.
 */
export interface DashboardEnvelope {
  runId: string;
  seq: number;
  ts: string;
  event: DashboardEvent;
  status: RunStatus;
  scheduler: SchedulerView;
  command?: CommandView;
}

export interface MonitorOptions {
  topology: CompositionIrNode;
  /** Display name, e.g. the workflow file name. */
  workflow?: string;
  runId?: string;
  capacity?: number;
  /** Envelopes retained for catch-up; a client further behind needs a fresh snapshot. */
  bufferSize?: number;
  now?: () => Date;
}

const TERMINAL: ReadonlySet<RunStatus> = new Set(["completed", "failed", "cancelled"]);

export function projectEvent(event: RunEvent): DashboardEvent {
  switch (event.type) {
    case "command.scheduled": {
      const { id, runnable, kind, concurrent, operation } = event.command;
      const target = operation.kind === "jssg" ? operation.target : undefined;
      return {
        type: "command.scheduled",
        commandId: id,
        runnable,
        kind,
        ...(concurrent ? { concurrent: true as const } : {}),
        ...(event.dynamicId === undefined ? {} : { dynamicId: event.dynamicId }),
        ...(target === undefined ? {} : { target }),
      };
    }
    case "command.replayed":
      return {
        type: "command.replayed",
        commandId: event.commandId,
        status: event.completion.status,
        ...(event.dynamicId === undefined ? {} : { dynamicId: event.dynamicId }),
      };
    case "command.completed": {
      const { completion } = event;
      if (completion.status === "succeeded") {
        return { type: "command.completed", commandId: event.commandId, status: "succeeded" };
      }
      const details = completion.error.details;
      const phase =
        typeof details === "object" && details !== null && !Array.isArray(details)
          ? details.phase
          : undefined;
      return {
        type: "command.completed",
        commandId: event.commandId,
        status: completion.status,
        error: completion.error.message,
        ...(typeof phase === "string" ? { phase } : {}),
      };
    }
    case "run.finished":
      return { type: "run.finished", replayed: event.replayed };
    default:
      return event;
  }
}

function operationNodes(
  node: CompositionIrNode,
  into = new Map<string, OperationIr>(),
): Map<string, OperationIr> {
  if (node.type === "operation") into.set(node.id, node);
  else if (node.type !== "dynamic") {
    for (const child of node.type === "sequence" ? node.stages : node.members) {
      operationNodes(child, into);
    }
  }
  return into;
}

export class RunMonitor implements EventSink {
  readonly runId: string;
  readonly #snapshot: DashboardSnapshot;
  readonly #commands = new Map<string, CommandView>();
  readonly #planned: Map<string, OperationIr>;
  readonly #buffer: DashboardEnvelope[] = [];
  readonly #bufferSize: number;
  readonly #listeners = new Set<(envelope: DashboardEnvelope) => void>();
  readonly #now: () => Date;

  constructor(options: MonitorOptions) {
    this.runId = options.runId ?? randomUUID();
    this.#now = options.now ?? (() => new Date());
    this.#bufferSize = options.bufferSize ?? 500;
    if (!Number.isInteger(this.#bufferSize) || this.#bufferSize < 1) {
      throw new Error(`monitor bufferSize must be a positive integer, got ${this.#bufferSize}`);
    }
    this.#planned = operationNodes(options.topology);
    this.#snapshot = {
      runId: this.runId,
      workflow: options.workflow ?? "workflow",
      startedAt: this.#now().toISOString(),
      status: "running",
      seq: 0,
      topology: structuredClone(options.topology),
      scheduler: {
        ...(options.capacity === undefined ? {} : { capacity: options.capacity }),
        used: 0,
        active: 0,
        queued: 0,
        paused: false,
      },
      commands: [],
    };
  }

  emit(event: RunEvent): void {
    this.#push(projectEvent(event));
  }

  /**
   * The outcome only the host sees: `run()` rejected. `cancelled` when the
   * host's abort signal had fired, `failed` otherwise. Ignored once terminal.
   */
  fail(message: string, cancelled = false): void {
    if (this.finished) return;
    this.#push({ type: cancelled ? "run.cancelled" : "run.failed", message });
  }

  get status(): RunStatus {
    return this.#snapshot.status;
  }

  get finished(): boolean {
    return TERMINAL.has(this.#snapshot.status);
  }

  get startedAt(): string {
    return this.#snapshot.startedAt;
  }

  get finishedAt(): string | undefined {
    return this.#snapshot.finishedAt;
  }

  /** Why a failed or cancelled run ended. */
  get error(): string | undefined {
    return this.#snapshot.error;
  }

  snapshot(): DashboardSnapshot {
    return structuredClone(this.#snapshot);
  }

  /**
   * Envelopes after `seq`, oldest first, or `undefined` when the buffer no
   * longer reaches back that far and the client must take a fresh snapshot.
   */
  since(seq: number): DashboardEnvelope[] | undefined {
    if (seq >= this.#snapshot.seq) return [];
    const oldest = this.#buffer[0]?.seq;
    if (oldest === undefined || oldest > seq + 1) return undefined;
    return this.#buffer.filter((envelope) => envelope.seq > seq);
  }

  subscribe(listener: (envelope: DashboardEnvelope) => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  #push(event: DashboardEvent): void {
    const ts = this.#now().toISOString();
    const command = this.#apply(event, ts);
    const snapshot = this.#snapshot;
    snapshot.seq += 1;
    const envelope: DashboardEnvelope = {
      runId: this.runId,
      seq: snapshot.seq,
      ts,
      event,
      status: snapshot.status,
      scheduler: { ...snapshot.scheduler },
      ...(command === undefined ? {} : { command: structuredClone(command) }),
    };
    this.#buffer.push(envelope);
    if (this.#buffer.length > this.#bufferSize) this.#buffer.shift();
    for (const listener of this.#listeners) {
      try {
        listener(envelope);
      } catch {
        // A subscriber's failure is its own; the run and the other subscribers continue.
      }
    }
  }

  #command(id: string, ts: string): CommandView {
    let view = this.#commands.get(id);
    if (view === undefined) {
      const planned = this.#planned.get(id);
      view = {
        id,
        ...(planned === undefined ? {} : { runnable: planned.name, kind: planned.kind }),
        state: "scheduled",
        static: planned !== undefined,
        scheduledAt: ts,
      };
      this.#commands.set(id, view);
      this.#snapshot.commands.push(view);
    }
    return view;
  }

  /** Fold one event in and return the command it changed, if any. */
  #apply(event: DashboardEvent, ts: string): CommandView | undefined {
    const snapshot = this.#snapshot;
    const scheduler = snapshot.scheduler;
    switch (event.type) {
      case "command.scheduled": {
        const view = this.#command(event.commandId, ts);
        view.runnable = event.runnable;
        view.kind = event.kind;
        view.state = "scheduled";
        if (event.concurrent) view.concurrent = true;
        if (event.dynamicId !== undefined) view.dynamicId = event.dynamicId;
        if (event.target !== undefined) view.target = event.target;
        return view;
      }
      case "scheduler.queued": {
        const view = this.#command(event.commandId, ts);
        view.state = "queued";
        view.weight = event.weight;
        scheduler.queued += 1;
        return view;
      }
      case "scheduler.admitted": {
        const view = this.#command(event.commandId, ts);
        if (view.state === "queued") scheduler.queued = Math.max(0, scheduler.queued - 1);
        view.state = "running";
        view.weight = event.weight;
        view.startedAt = ts;
        scheduler.capacity = event.capacity;
        scheduler.active = event.active;
        scheduler.used = event.used;
        return view;
      }
      case "scheduler.released": {
        scheduler.active = event.active;
        scheduler.used = event.used;
        return this.#command(event.commandId, ts);
      }
      case "bridge.spawned": {
        const view = this.#command(event.commandId, ts);
        if (event.pid !== undefined) view.pid = event.pid;
        return view;
      }
      case "command.completed": {
        const view = this.#command(event.commandId, ts);
        if (view.state === "queued") scheduler.queued = Math.max(0, scheduler.queued - 1);
        view.state = event.status;
        view.finishedAt = ts;
        if (event.error !== undefined) view.error = event.error;
        if (event.phase !== undefined) view.phase = event.phase;
        return view;
      }
      case "command.replayed": {
        const view = this.#command(event.commandId, ts);
        view.state = "replayed";
        view.status = event.status;
        if (event.dynamicId !== undefined) view.dynamicId = event.dynamicId;
        view.finishedAt = ts;
        return view;
      }
      case "scheduler.paused":
        scheduler.paused = true;
        scheduler.queued = event.queued;
        scheduler.active = event.active;
        if (!this.finished) snapshot.status = "paused";
        return undefined;
      case "scheduler.resumed":
        scheduler.paused = false;
        scheduler.queued = event.queued;
        scheduler.active = event.active;
        if (!this.finished) snapshot.status = "running";
        return undefined;
      case "run.finished":
        snapshot.status = "completed";
        snapshot.replayed = event.replayed;
        snapshot.finishedAt = ts;
        return undefined;
      case "run.failed":
      case "run.cancelled":
        snapshot.status = event.type === "run.failed" ? "failed" : "cancelled";
        snapshot.error = event.message;
        snapshot.finishedAt = ts;
        return undefined;
    }
  }
}

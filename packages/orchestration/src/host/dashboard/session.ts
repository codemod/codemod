/**
 * The dashboard's run manager: one loaded workflow configuration and every
 * run of it this process has made.
 *
 * A session hands each run fresh collaborators: a `RunMonitor` under a new
 * run id (its own sequence numbers, timestamps, and final snapshot), an
 * `AdmissionScheduler`, an `AbortController`, and a `run()` over an empty
 * history store, so nothing is ever replayed from an earlier run and no
 * `ReplayGate` history crosses between runs. At most one run is active at a
 * time. `start()` refuses while one is; `restart()` aborts the active run,
 * waits for it to settle (every bridge process gone, every completion
 * recorded), and only then launches the next, so the side effects of two runs
 * never overlap. Every transition goes through one serial chain, so a double
 * tap queues behind the first and then sees the run it would have duplicated.
 *
 * Retained records are process-local and bounded: newest first, at most
 * `runLimit`, and a run that has not settled is never evicted. They are not
 * history in the replay sense and they are not durable. Nothing here knows
 * about HTTP or the page; `api.ts` and `server.ts` sit on top.
 */
import { randomUUID } from "node:crypto";
import {
  executableIr,
  type CompositionIrNode,
  type Executable,
} from "../../authoring/composition.ts";
import type { EventSink } from "../../core/events.ts";
import type { Json } from "../../core/json.ts";
import type { OperationExecutor } from "../../execution/executor.ts";
import { AdmissionScheduler, defaultCapacity } from "../../execution/scheduler.ts";
import { run } from "../../runtime/run.ts";
import {
  RunMonitor,
  type DashboardSnapshot,
  type RunStatus,
  type SchedulerView,
} from "./monitor.ts";

/** Run records kept in memory, newest first. */
export const DEFAULT_RUN_LIMIT = 20;

export interface SessionOptions {
  executable: Executable;
  /** Display name, e.g. the workflow file name. */
  workflow?: string;
  /**
   * Builds the executor for one run. Called once per run with that run's
   * event sink, so `bridge.spawned` reaches the right monitor.
   */
  executor: (events: EventSink) => OperationExecutor;
  /** The root input. The key's presence matters: `{ input: null }` is the value `null`. */
  input?: Json;
  /** Admission capacity shared by every run's scheduler. Defaults to `defaultCapacity()`. */
  capacity?: number;
  /** Defaults to `DEFAULT_RUN_LIMIT`. */
  runLimit?: number;
  /** Per-run envelope buffer for reconnect catch-up; see `MonitorOptions`. */
  bufferSize?: number;
  runId?: () => string;
  now?: () => Date;
}

export interface RunSummary {
  runId: string;
  /** 1-based, in order of creation within this session. */
  number: number;
  status: RunStatus;
  startedAt: string;
  finishedAt?: string;
  /** Milliseconds from start to finish, or to now while the run is open. */
  durationMs: number;
  /** Why a failed or cancelled run ended. */
  error?: string;
}

/** What the session tells subscribers; the stream forwards these so a page follows a new run without reloading. */
export type SessionNotice =
  | { type: "run.created"; run: RunSummary; stopped?: string }
  | { type: "run.settled"; run: RunSummary };

export interface RestartResult {
  run: RunSummary;
  /** The run that was aborted to make room, when there was one. */
  stopped?: string;
}

export interface ControlResult {
  runId: string;
  status: RunStatus;
  scheduler: SchedulerView;
}

/** The host-only outcome of a settled run: `run()`'s output or its rejection. */
export type RunOutcome = { runId: string; number: number } & (
  | { output: unknown }
  | { error: unknown }
);

export type ConflictCode = "run_active" | "no_active_run" | "session_closed";

/** A transition the session refuses in its current state; `409` over HTTP. */
export class RunConflictError extends Error {
  readonly httpStatus = 409;
  constructor(
    readonly code: ConflictCode,
    message: string,
    readonly runId?: string,
    readonly runStatus?: RunStatus,
  ) {
    super(message);
    this.name = "RunConflictError";
  }
}

/** A run id this session no longer (or never) retains; `404` over HTTP. */
export class RunNotFoundError extends Error {
  readonly httpStatus = 404;
  readonly code = "run_not_found";
  constructor(readonly runId: string) {
    super(`no run '${runId}' is retained`);
    this.name = "RunNotFoundError";
  }
}

interface RunRecord {
  number: number;
  monitor: RunMonitor;
  scheduler: AdmissionScheduler;
  controller: AbortController;
  /** `run()` has settled: no operation is in flight and the outcome is known. */
  done: boolean;
  settled: Promise<void>;
  outcome?: { output: unknown } | { error: unknown };
}

export class DashboardSession {
  readonly workflow: string;
  readonly topology: CompositionIrNode;
  readonly capacity: number;
  /** Resolves once `close()` has finished: no run is active and none can start. */
  readonly closed: Promise<void>;
  readonly #options: SessionOptions;
  readonly #limit: number;
  /** Newest first. */
  readonly #records: RunRecord[] = [];
  readonly #listeners = new Set<(notice: SessionNotice) => void>();
  readonly #now: () => Date;
  readonly #newRunId: () => string;
  #counter = 0;
  #chain: Promise<unknown> = Promise.resolve();
  #restarting: Promise<RestartResult> | undefined;
  #isClosed = false;
  #closing: Promise<void> | undefined;
  #resolveClosed!: () => void;

  constructor(options: SessionOptions) {
    this.#options = options;
    this.workflow = options.workflow ?? "workflow";
    this.topology = executableIr(options.executable);
    this.capacity = options.capacity ?? defaultCapacity();
    this.#limit = options.runLimit ?? DEFAULT_RUN_LIMIT;
    if (!Number.isInteger(this.#limit) || this.#limit < 1) {
      throw new Error(`session runLimit must be a positive integer, got ${this.#limit}`);
    }
    this.#now = options.now ?? (() => new Date());
    this.#newRunId = options.runId ?? (() => randomUUID());
    this.closed = new Promise<void>((resolve) => {
      this.#resolveClosed = resolve;
    });
  }

  get isClosed(): boolean {
    return this.#isClosed;
  }

  /** The newest run; the one live views follow. */
  get currentRunId(): string | undefined {
    return this.#records[0]?.monitor.runId;
  }

  /** The run that has not settled yet, if any. Controls act on it alone. */
  get activeRunId(): string | undefined {
    return this.#active()?.monitor.runId;
  }

  /** Newest first. */
  runs(): RunSummary[] {
    return this.#records.map((record) => this.#summary(record));
  }

  run(runId: string): RunSummary | undefined {
    const record = this.#find(runId);
    return record === undefined ? undefined : this.#summary(record);
  }

  /** The run's current or final snapshot; the newest run by default. */
  snapshot(runId = this.currentRunId): DashboardSnapshot | undefined {
    return runId === undefined ? undefined : this.#find(runId)?.monitor.snapshot();
  }

  /** Read-only access to a retained run's monitor, for streaming catch-up. */
  monitor(runId: string): RunMonitor | undefined {
    return this.#find(runId)?.monitor;
  }

  /** Defined once the run has settled; the newest run by default. */
  outcome(runId = this.currentRunId): RunOutcome | undefined {
    const record = runId === undefined ? undefined : this.#find(runId);
    if (record?.outcome === undefined) return undefined;
    return { runId: record.monitor.runId, number: record.number, ...record.outcome };
  }

  /** Resolves when the run has settled; the newest run by default. */
  settled(runId = this.currentRunId): Promise<void> {
    if (runId === undefined) return Promise.resolve();
    const record = this.#find(runId);
    if (record === undefined) throw new RunNotFoundError(runId);
    return record.settled;
  }

  /**
   * Launch a fresh run. Refused with `run_active` while a run is active; a
   * run that has emitted its final event but not yet settled is waited for.
   */
  start(): Promise<RunSummary> {
    return this.#serialize(async () => {
      this.#refuseIfClosed();
      const active = this.#active();
      if (active !== undefined) {
        if (!active.monitor.finished) {
          throw new RunConflictError(
            "run_active",
            "a run is active; use Restart to stop it and start over",
            active.monitor.runId,
            active.monitor.status,
          );
        }
        await active.settled;
        this.#refuseIfClosed();
      }
      return this.#launch().run;
    });
  }

  /**
   * Abort the active run if there is one, wait for it to settle, then launch a
   * fresh run. A restart already in progress is returned to a second caller
   * rather than repeated.
   */
  restart(): Promise<RestartResult> {
    if (this.#restarting !== undefined) return this.#restarting;
    const restarting = this.#serialize(async () => {
      this.#refuseIfClosed();
      const active = this.#active();
      if (active === undefined) return this.#launch();
      active.controller.abort();
      await active.settled;
      this.#refuseIfClosed();
      return this.#launch(active.monitor.runId);
    }).finally(() => {
      this.#restarting = undefined;
    });
    this.#restarting = restarting;
    return restarting;
  }

  pause(): ControlResult {
    return this.#control((record) => record.scheduler.pause());
  }

  resume(): ControlResult {
    return this.#control((record) => record.scheduler.resume());
  }

  subscribe(listener: (notice: SessionNotice) => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * Refuse further transitions, abort the active run, and resolve once it has
   * settled. Idempotent; `closed` resolves at the same moment.
   */
  close(): Promise<void> {
    if (this.#closing === undefined) {
      this.#isClosed = true;
      this.#closing = this.#serialize(async () => {
        const active = this.#active();
        if (active === undefined) return;
        active.controller.abort();
        await active.settled;
      }).finally(() => this.#resolveClosed());
    }
    return this.#closing;
  }

  #serialize<T>(step: () => Promise<T>): Promise<T> {
    const next = this.#chain.then(step, step);
    this.#chain = next.catch(() => {});
    return next;
  }

  #refuseIfClosed(): void {
    if (this.#isClosed) {
      throw new RunConflictError("session_closed", "the dashboard session is closed");
    }
  }

  #find(runId: string): RunRecord | undefined {
    return this.#records.find((record) => record.monitor.runId === runId);
  }

  #active(): RunRecord | undefined {
    return this.#records.find((record) => !record.done);
  }

  #control(action: (record: RunRecord) => void): ControlResult {
    const active = this.#active();
    if (active === undefined || active.monitor.finished) {
      const current = this.#records[0];
      throw new RunConflictError(
        "no_active_run",
        current === undefined ? "no run has started" : `run already ${current.monitor.status}`,
        current?.monitor.runId,
        current?.monitor.status,
      );
    }
    action(active);
    return {
      runId: active.monitor.runId,
      status: active.monitor.status,
      scheduler: active.monitor.snapshot().scheduler,
    };
  }

  #launch(stopped?: string): RestartResult {
    const runId = this.#newRunId();
    if (this.#find(runId) !== undefined) throw new Error(`run id '${runId}' was issued twice`);
    const { bufferSize } = this.#options;
    const monitor = new RunMonitor({
      topology: this.topology,
      workflow: this.workflow,
      runId,
      capacity: this.capacity,
      ...(bufferSize === undefined ? {} : { bufferSize }),
      now: this.#now,
    });
    const record: RunRecord = {
      number: ++this.#counter,
      monitor,
      scheduler: new AdmissionScheduler({ capacity: this.capacity, events: monitor }),
      controller: new AbortController(),
      done: false,
      settled: Promise.resolve(),
    };
    this.#records.unshift(record);
    this.#evict();
    record.settled = this.#execute(record);
    const run = this.#summary(record);
    const result: RestartResult = stopped === undefined ? { run } : { run, stopped };
    this.#notify({ type: "run.created", ...result });
    return result;
  }

  async #execute(record: RunRecord): Promise<void> {
    const { monitor, scheduler, controller } = record;
    const options = this.#options;
    try {
      // No `history`: a fresh in-memory store, so nothing from an earlier run replays.
      const result = await run(options.executable, {
        executor: options.executor(monitor),
        events: monitor,
        scheduler,
        signal: controller.signal,
        ...("input" in options ? { input: options.input } : {}),
      });
      record.outcome = { output: result.output };
      if (!monitor.finished) {
        monitor.emit({ type: "run.finished", output: null, replayed: result.replayed });
      }
    } catch (error) {
      record.outcome = { error };
      monitor.fail(
        error instanceof Error ? error.message : String(error),
        controller.signal.aborted,
      );
    } finally {
      record.done = true;
    }
    this.#notify({ type: "run.settled", run: this.#summary(record) });
  }

  /** Drop the oldest settled records beyond the limit; an unsettled run is never dropped. */
  #evict(): void {
    while (this.#records.length > this.#limit) {
      const oldest = this.#records.at(-1)!;
      if (!oldest.done) break;
      this.#records.pop();
    }
  }

  #summary(record: RunRecord): RunSummary {
    const { monitor } = record;
    const finishedAt = monitor.finishedAt;
    const end = finishedAt ?? this.#now().toISOString();
    const error = monitor.error;
    return {
      runId: monitor.runId,
      number: record.number,
      status: monitor.status,
      startedAt: monitor.startedAt,
      ...(finishedAt === undefined ? {} : { finishedAt }),
      durationMs: Math.max(0, Date.parse(end) - Date.parse(monitor.startedAt)),
      ...(error === undefined ? {} : { error }),
    };
  }

  #notify(notice: SessionNotice): void {
    for (const listener of this.#listeners) {
      try {
        listener(notice);
      } catch {
        // A subscriber's failure is its own.
      }
    }
  }
}

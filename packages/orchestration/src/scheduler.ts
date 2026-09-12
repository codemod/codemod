/**
 * Bounded admission for operations.
 *
 * `parallel()` is an author assertion that members may overlap; it never says
 * how many may run at once. That is the runtime's business, so every run owns
 * one `AdmissionScheduler` and every executed operation must acquire a permit
 * from it before the executor is called. Replayed commands never reach the
 * executor, so they consume no capacity.
 *
 * The model is a weighted semaphore with a strict FIFO queue. Weights make one
 * workspace-semantic JSSG command, which reads and retains the whole selected
 * file set and then has a bridge process parse and index it, cost more than an
 * ordinary `exec`. Strict FIFO (only the head of the queue may be admitted)
 * costs some utilization when a heavy command blocks lighter ones behind it,
 * and buys the guarantee that a heavy command is never starved.
 *
 * The permit is taken around `OperationExecutor.execute`, which is also where
 * JSSG selection and file reading happen (`jssg.ts`), so a queued command
 * retains only its `OperationRequest`: operation metadata, never a repository
 * snapshot.
 */
import { availableParallelism, totalmem } from "node:os";
import { nullSink, type EventSink } from "./events.ts";
import type { OperationExecutor } from "./executor.ts";
import {
  PROTOCOL_VERSION,
  type Operation,
  type OperationCompletion,
  type OperationRequest,
  type SemanticAnalysis,
} from "./protocol.ts";

/**
 * Cost of one admitted operation, in units where 1 unit is roughly one
 * ordinary command holding one CPU.
 */
export interface OperationWeights {
  exec: number;
  ai: number;
  /** A JSSG batch: a bridge process plus the whole selected file set in memory. */
  jssg: number;
  /** Workspace semantics additionally index the whole batch; the dominant cost. */
  jssgWorkspace: number;
}

export const DEFAULT_WEIGHTS: OperationWeights = {
  exec: 1,
  ai: 1,
  jssg: 2,
  jssgWorkspace: 4,
};

/** The host facts the default capacity is derived from; injectable for tests. */
export interface SchedulerHost {
  availableParallelism(): number;
  totalmem(): number;
}

export const nodeHost: SchedulerHost = { availableParallelism, totalmem };

/**
 * Operator/CI override for the total capacity, in the same units as the
 * weights. It is host configuration: it never reaches a workflow module and is
 * not part of any command's identity.
 */
export const CAPACITY_ENV = "CODEMOD_ORCHESTRATION_CAPACITY";

/** Share of host memory the whole run may plan around. */
const MEMORY_BUDGET_FRACTION = 0.5;

/**
 * Memory one concurrent workspace-semantic pass is assumed to need: the batch
 * as host strings, the same batch as request JSON, and the bridge's parse and
 * semantic index of it.
 */
const WORKSPACE_FOOTPRINT_BYTES = 512 * 1024 * 1024;

export interface SchedulerOptions {
  /** Total capacity in weight units. Defaults to `defaultCapacity()`. */
  capacity?: number;
  weights?: Partial<OperationWeights>;
  /** Defaults to `nodeHost`. */
  host?: SchedulerHost;
  env?: Record<string, string | undefined>;
}

/** What the scheduler is doing right now; a deterministic hook for tests and benchmarks. */
export interface SchedulerStats {
  capacity: number;
  /** Weight currently held by admitted operations. */
  used: number;
  /** Operations currently admitted. */
  active: number;
  queued: number;
  /** Highest `active` reached during this run. */
  peakActive: number;
  /** Highest `used` reached during this run. */
  peakUsed: number;
}

export interface Permit {
  readonly weight: number;
  /** Idempotent: releasing twice does not return capacity twice. */
  release(): void;
}

const isWorkspace = (semantic: SemanticAnalysis | undefined): boolean =>
  semantic === "workspace" || (typeof semantic === "object" && semantic.mode === "workspace");

export function weightOf(
  operation: Operation,
  weights: OperationWeights = DEFAULT_WEIGHTS,
): number {
  switch (operation.kind) {
    case "exec":
      return weights.exec;
    case "ai":
      return weights.ai;
    case "jssg":
      return isWorkspace(operation.semanticAnalysis) ? weights.jssgWorkspace : weights.jssg;
  }
}

/**
 * Bounded by CPU and memory. A heavy operation is clamped to the available
 * capacity when admitted, so small hosts do not need an artificial capacity
 * floor just to make progress.
 */
export function defaultCapacity(
  weights: OperationWeights = DEFAULT_WEIGHTS,
  host: SchedulerHost = nodeHost,
  env: Record<string, string | undefined> = process.env,
): number {
  const heaviest = Math.max(...Object.values(weights));
  const override = env[CAPACITY_ENV];
  if (override !== undefined && override.trim() !== "") {
    const parsed = Number(override);
    if (!Number.isInteger(parsed) || parsed < 1) {
      throw new Error(`${CAPACITY_ENV} must be a positive integer, got '${override}'`);
    }
    return parsed;
  }
  const cpu = Math.max(1, host.availableParallelism());
  const workspacePasses = Math.floor(
    (host.totalmem() * MEMORY_BUDGET_FRACTION) / WORKSPACE_FOOTPRINT_BYTES,
  );
  return Math.max(1, Math.min(cpu, Math.max(1, workspacePasses) * heaviest));
}

interface Waiter {
  commandId: string;
  weight: number;
  events: EventSink;
  settle(permit: Permit | undefined): void;
}

/**
 * One run's admission control. Not global: two concurrent runs bound
 * themselves independently, exactly as two host processes would.
 */
export class AdmissionScheduler {
  readonly capacity: number;
  readonly weights: OperationWeights;
  readonly #queue: Waiter[] = [];
  #used = 0;
  #active = 0;
  #peakActive = 0;
  #peakUsed = 0;

  constructor(options: SchedulerOptions = {}) {
    this.weights = { ...DEFAULT_WEIGHTS, ...options.weights };
    this.capacity =
      options.capacity ?? defaultCapacity(this.weights, options.host, options.env ?? process.env);
    if (!Number.isInteger(this.capacity) || this.capacity < 1) {
      throw new Error(`scheduler capacity must be a positive integer, got ${this.capacity}`);
    }
  }

  stats(): SchedulerStats {
    return {
      capacity: this.capacity,
      used: this.#used,
      active: this.#active,
      queued: this.#queue.length,
      peakActive: this.#peakActive,
      peakUsed: this.#peakUsed,
    };
  }

  weightFor(operation: Operation): number {
    return this.#cost(weightOf(operation, this.weights));
  }

  /**
   * Resolves with a permit once the operation may run, or with `undefined`
   * when `signal` aborted while it was still queued. An already-aborted signal
   * is refused without taking capacity, so nothing is spawned for it.
   *
   * `events` belongs to the caller's run rather than to the scheduler, so
   * scheduling events reach the sink of whoever asked for the permit.
   */
  acquire(
    commandId: string,
    weight: number,
    signal?: AbortSignal,
    events: EventSink = nullSink,
  ): Promise<Permit | undefined> {
    const cost = this.#cost(weight);
    if (signal?.aborted) return Promise.resolve(undefined);
    if (this.#queue.length === 0 && this.#used + cost <= this.capacity) {
      return Promise.resolve(this.#admit(commandId, cost, events));
    }
    events.emit({ type: "scheduler.queued", commandId, weight: cost });
    return new Promise<Permit | undefined>((resolve) => {
      const onAbort = () => {
        const index = this.#queue.indexOf(waiter);
        if (index === -1) return;
        this.#queue.splice(index, 1);
        resolve(undefined);
        // Removing a waiter can unblock the operations behind it.
        this.#pump();
      };
      const waiter: Waiter = {
        commandId,
        weight: cost,
        events,
        settle: (permit) => {
          signal?.removeEventListener("abort", onAbort);
          resolve(permit);
        },
      };
      signal?.addEventListener("abort", onAbort, { once: true });
      this.#queue.push(waiter);
    });
  }

  /** Clamp to the range a permit can actually be granted in. */
  #cost(weight: number): number {
    return Math.min(Math.max(1, Math.round(weight)), this.capacity);
  }

  #admit(commandId: string, cost: number, events: EventSink): Permit {
    this.#used += cost;
    this.#active += 1;
    this.#peakActive = Math.max(this.#peakActive, this.#active);
    this.#peakUsed = Math.max(this.#peakUsed, this.#used);
    events.emit({
      type: "scheduler.admitted",
      commandId,
      weight: cost,
      active: this.#active,
      used: this.#used,
      capacity: this.capacity,
    });
    let released = false;
    return {
      weight: cost,
      release: () => {
        if (released) return;
        released = true;
        this.#used -= cost;
        this.#active -= 1;
        events.emit({
          type: "scheduler.released",
          commandId,
          weight: cost,
          active: this.#active,
          used: this.#used,
        });
        this.#pump();
      },
    };
  }

  /** Admit from the head only, so a heavy operation is never overtaken forever. */
  #pump(): void {
    while (this.#queue.length > 0 && this.#used + this.#queue[0]!.weight <= this.capacity) {
      const waiter = this.#queue.shift()!;
      waiter.settle(this.#admit(waiter.commandId, waiter.weight, waiter.events));
    }
  }
}

/**
 * The execution boundary: an `OperationExecutor` that admits through a
 * scheduler before delegating. Every plan and workflow benefits because
 * `run()` wraps the executor it is given, and nothing below this point is
 * aware of scheduling.
 *
 * A command aborted while still queued is refused here, so no bridge process
 * is ever spawned for it. A command that was already admitted keeps the
 * existing behavior: the inner executor observes the same signal and kills its
 * child.
 */
export class SchedulingExecutor implements OperationExecutor {
  constructor(
    private readonly inner: OperationExecutor,
    readonly scheduler: AdmissionScheduler,
    private readonly events: EventSink = nullSink,
  ) {}

  async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    const permit = await this.scheduler.acquire(
      request.commandId,
      this.scheduler.weightFor(request.operation),
      signal,
      this.events,
    );
    if (permit === undefined) {
      return {
        protocolVersion: PROTOCOL_VERSION,
        commandId: request.commandId,
        status: "cancelled",
        error: { message: "cancelled while queued for execution capacity" },
      };
    }
    try {
      return await this.inner.execute(request, signal);
    } finally {
      // Covers success, a non-success completion, a thrown launch error, and abort.
      permit.release();
    }
  }
}

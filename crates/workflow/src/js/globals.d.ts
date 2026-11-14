// What the engine (crates/workflow/src/run.rs, `install_host`) puts on the
// global object of the workflow body's QuickJS context, and what the modules
// here put there for it. Types only: nothing in this file runs.

/** The host as Rust exposes it. Everything structured crosses as JSON text. */
interface RawHost {
  /** Steps recorded by earlier executions of the run: `StepRecord[]`. */
  history(): string;
  /** Persist one step's outcome (`StepRecord`); `ms` is how long it took. */
  record(entry: string, ms: number): void;
  /** Report a step started, replayed or retrying (`StepEvent`). */
  event(event: string): void;
  /** Whether a step about to start must not, because the run was asked to stop. */
  canceled(): boolean;
  /** Start a task (`StartRequest`); the engine settles it with `__workflowSettle`. */
  start(request: string): number;
  /** Evaluate a workflow.yaml `${{ }}` expression; returns JSON. */
  evaluate(kind: string, expression: string, scope: string): string;
  /** The real time, in milliseconds since the epoch. */
  now(): number;
  /** The body's clock: the run's start time, moved on as steps finish. */
  clock(): number;
  /** Move the body's clock forward to `ms`. */
  advanceClock(ms: number): void;
  /** A seed for the body's random numbers, the same on every resume. */
  seed(): number;
  /** A line the body wrote to the console. */
  log(line: string): void;
  /** `TestPlan` as JSON when the run is a test, else `"null"`. */
  testPlan(): string;
  /** The test's `until` step has started. */
  stop(): void;
}

// `var` is how a global is declared; these are properties of `globalThis`.
/* eslint-disable no-var */
declare var __workflowHost: RawHost;
/** Settle the task `id` with its outcome, or its error payload, as JSON. */
declare var __workflowSettle: (id: number, ok: boolean, payload: string) => void;
/** Settle the next replayed step; false when none is waiting. */
declare var __workflowReplayNext: () => boolean;
/** No step starts from here on. */
declare var __workflowClose: () => void;
/* eslint-enable no-var */

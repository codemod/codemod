// Which run, and which step of it, the code running now belongs to. The
// `codemod/*` modules are singletons, and runs may overlap (tests in
// parallel), so they look the run up here.
//
// The store is shared through a global symbol, so it stays one store when a
// test runner loads this package twice (once natively, once through its own
// module graph).

import { AsyncLocalStorage } from "node:async_hooks";

import type { NativeRun } from "./addon.mts";
import type { Runtime } from "./orchestrator.mts";

/** What a step or `forEachFile` callback sees and leaves behind. */
export interface StepScope {
  position: number;
  label: string;
  /** A failure reported with `failFile` or `failStep`. */
  failure: string | undefined;
  /** `parse()`'s default language. */
  language: string | undefined;
}

/** The run the code running now belongs to, and its step when it is in one. */
export interface RunScope {
  run: NativeRun;
  runtime: Runtime;
  /** Whether `console` output of the run's code goes to its logs. */
  captureConsole: boolean;
  /** What `Date`, `performance.now` and `Math.random` give in the body. */
  body?: BodyClock;
  step?: StepScope;
}

/**
 * The workflow body's clock and random numbers: the clock starts at the
 * run's start time and only moves to the time recorded for a step that
 * finished; the numbers are seeded by the run id. So the body issues the
 * same steps on every resume.
 */
export interface BodyClock {
  /** The workflow clock, in ms since the epoch. */
  now(): number;
  /** Where the clock started: the run's start time. */
  readonly origin: number;
  /** Move the clock to `ms`, if that is later. */
  advance(ms: number): void;
  /** The next seeded number in [0, 1). */
  random(): number;
}

const KEY = Symbol.for("codemod.workflow.context");
const shared = globalThis as { [KEY]?: AsyncLocalStorage<RunScope> };

export const context: AsyncLocalStorage<RunScope> = (shared[KEY] ??= new AsyncLocalStorage());

/** The run the calling code belongs to; throws outside a run. */
export function current(what: string): RunScope {
  const store = context.getStore();
  if (!store) {
    throw new Error(
      `${what} only works while a workflow runs; run the workflow with codemod (the CLI, or runWorkflow from codemod/testing)`,
    );
  }
  return store;
}

/** A fresh step scope. */
export function stepScope(position: number, label: string, language?: string): StepScope {
  return { position, label, failure: undefined, language };
}

// Which run, and which step of it, the code running now belongs to. The
// `codemod/*` modules are singletons, and runs may overlap (tests in
// parallel), so they look the run up here.
//
// The store is shared through a global symbol, so it stays one store when a
// test runner loads this package twice (once natively, once through its own
// module graph).

import { AsyncLocalStorage } from "node:async_hooks";

import type { NativeRun } from "./addon.mts";
import type { Json, Runtime } from "./orchestrator.mts";

/** One count of a metric, for one combination of labels. */
export type MetricEntry = {
  cardinality: Record<string, string>;
  count: number;
};

/** What a step or `forEachFile` callback sees and leaves behind. */
export interface StepScope {
  position: number;
  label: string;
  /** State as the step sees it. */
  state: Map<string, unknown>;
  /** The state the step started with, as JSON, by key. */
  initial: Map<string, string>;
  /** Keys set with `persist: false`: the step's own. */
  unpersisted: Set<string>;
  /** Keys the step removed. */
  removed: Set<string>;
  /** Step outputs set here. */
  outputs: Map<string, string>;
  /** Metrics by name, then by their labels as JSON. */
  metrics: Map<string, Map<string, MetricEntry>>;
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
  step?: StepScope;
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

/** A fresh step scope, starting from the workflow state the step began with. */
export function stepScope(
  position: number,
  label: string,
  state: Record<string, Json> | undefined,
  language?: string,
): StepScope {
  const entries = Object.entries(state ?? {});
  return {
    position,
    label,
    state: new Map(entries.map(([key, value]) => [key, structuredClone(value)])),
    initial: new Map(entries.map(([key, value]) => [key, JSON.stringify(value)])),
    unpersisted: new Set(),
    removed: new Set(),
    outputs: new Map(),
    metrics: new Map(),
    failure: undefined,
    language,
  };
}

/**
 * The state a step set or removed (`null`), compared with the state it
 * started with: what the engine's isolates report for the same code.
 */
export function stateChanges(scope: StepScope): Record<string, Json> {
  const changes: Record<string, Json> = {};
  for (const [key, value] of scope.state) {
    if (scope.unpersisted.has(key)) continue;
    // What a step sets was made JSON when it was set (`stateValue`).
    if (scope.initial.get(key) !== JSON.stringify(value)) changes[key] = value as Json;
  }
  for (const key of scope.removed) changes[key] = null;
  return changes;
}

/** The metrics a step recorded, as the engine reports them. */
export function metricsOf(scope: StepScope): Record<string, MetricEntry[]> {
  const data: Record<string, MetricEntry[]> = {};
  for (const [name, entries] of scope.metrics) data[name] = [...entries.values()];
  return data;
}

// Which run, and which step of it, the code running now belongs to. The
// `codemod:*` modules are singletons, and runs may overlap (tests in
// parallel), so they look the run up here.
//
// The store is shared through a global symbol, so it stays one store when a
// test runner loads this package twice (once natively, once through its own
// module graph).

import { AsyncLocalStorage } from "node:async_hooks";

const KEY = Symbol.for("codemod.workflow.context");

/** @type {AsyncLocalStorage<{ run: any, runtime: any, step?: StepScope }>} */
export const context = (globalThis[KEY] ??= new AsyncLocalStorage());

/**
 * @typedef {object} StepScope
 * @property {number} position
 * @property {string} label
 * @property {Map<string, unknown>} state   state as the step sees it
 * @property {Record<string, unknown>} changes  state the step set; `null` removes
 * @property {Map<string, Map<string, { cardinality: Record<string, string>, count: number }>>} metrics
 * @property {string | undefined} language  `parse()`'s default language
 */

/** The run the calling code belongs to; throws outside a run. */
export function current(what) {
  const store = context.getStore();
  if (!store) {
    throw new Error(
      `${what} only works while a workflow runs; run the workflow with codemod (the CLI, or runWorkflow from codemod/testing)`,
    );
  }
  return store;
}

/** A fresh step scope, starting from the workflow state the step began with. */
export function stepScope(position, label, state, language) {
  return {
    position,
    label,
    state: new Map(Object.entries(state ?? {})),
    changes: {},
    metrics: new Map(),
    language,
  };
}

/** The metrics a step recorded, as the engine reports them. */
export function metricsOf(scope) {
  const data = {};
  for (const [name, entries] of scope.metrics) data[name] = [...entries.values()];
  return data;
}

// `codemod/metrics` in Node.js and Bun. Metrics belong to the step that
// records them: they go into a `forEachFile` step's result and the run's
// report. The workflow body cannot record them, as in the engine.

import { context } from "./context.mjs";

class MetricAtom {
  constructor(name) {
    this.name = name;
  }

  increment(cardinality = {}, amount = 1) {
    const scope = context.getStore()?.step;
    if (!scope) {
      throw new Error(
        `metric '${this.name}': metrics can only be recorded inside a step or forEachFile callback`,
      );
    }
    const clean = {};
    for (const key of Object.keys(cardinality ?? {}).sort()) {
      const value = cardinality[key];
      if (value !== undefined && value !== null) clean[key] = String(value);
    }
    const entryKey = JSON.stringify(clean);
    let entries = scope.metrics.get(this.name);
    if (!entries) scope.metrics.set(this.name, (entries = new Map()));
    const entry = entries.get(entryKey);
    if (entry) entry.count += amount;
    else entries.set(entryKey, { cardinality: clean, count: amount });
  }

  getEntries() {
    const entries = context.getStore()?.step?.metrics.get(this.name);
    return entries ? [...entries.values()].map((entry) => ({ ...entry })) : [];
  }
}

export function useMetricAtom(name) {
  return new MetricAtom(name);
}

export { MetricAtom };

// `codemod/metrics` in Node.js and Bun. Metrics belong to the step that
// records them: they go into a `forEachFile` step's result and the run's
// report. The workflow body cannot record them, as in the engine.

import { context } from "./context.mts";
import type { MetricEntry } from "./context.mts";

class MetricAtom {
  readonly name: string;

  constructor(name: string) {
    this.name = name;
  }

  increment(cardinality: Record<string, unknown> | null = {}, amount: unknown = 1): void {
    const scope = context.getStore()?.step;
    if (!scope) {
      throw new Error(
        `metric '${this.name}': metrics can only be recorded inside a step or forEachFile callback`,
      );
    }
    const labels = cardinality ?? {};
    const clean: Record<string, string> = {};
    for (const key of Object.keys(labels).sort()) {
      const value = labels[key];
      if (value !== undefined && value !== null) clean[key] = String(value);
    }
    // A whole amount of at least 0; anything that is not a number counts 1.
    const count =
      typeof amount === "number" && Number.isFinite(amount) ? Math.max(0, Math.trunc(amount)) : 1;
    const entryKey = JSON.stringify(clean);
    let entries = scope.metrics.get(this.name);
    if (!entries) scope.metrics.set(this.name, (entries = new Map()));
    const entry = entries.get(entryKey);
    if (entry) entry.count += count;
    else entries.set(entryKey, { cardinality: clean, count });
  }

  getEntries(): MetricEntry[] {
    const entries = context.getStore()?.step?.metrics.get(this.name);
    return entries ? [...entries.values()].map((entry) => ({ ...entry })) : [];
  }
}

export function useMetricAtom(name: string): MetricAtom {
  return new MetricAtom(name);
}

export { MetricAtom };

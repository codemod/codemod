// `codemod-internal:v1`: the codemod 1.x transform APIs that 2.0 removed,
// for packages whose workflow is a workflow.yaml (1.x packages) and only
// for them: in their runs `codemod:metrics` resolves to this module, and
// `codemod:workflow` to the isolates' `codemod/workflow` plus the state
// functions below (crates/workflow/src/tasks/isolate.rs). Workflows written
// in TypeScript never get them.
//
// - `getState`, `setState`, `unsetState`, `acquireLock`: state the step's
//   files share through `__codemodV1Host` (tasks/v1_host.rs), on top of the
//   state earlier steps left, which the translated workflow passes in
//   (`jssgFile(..., { v1State: state })`, kept in `__codemodV1State`).
//   What a step sets is returned with its result, and the workflow body
//   copies it into its state, so later steps (shell steps too) see it.
// - `useMetricAtom`: counts the step's files share; returned with the step's
//   result, and added up into the run's output (`metrics`).
// - `setStepOutput`, `getStepOutput`, `getOrSetStepOutput`: not provided;
//   calling one says so.

interface Host {
  get(name: string): string | undefined;
  set(name: string, json: string, persist: boolean): void;
  unset(name: string): void;
  lock(name: string): void;
  unlock(name: string): void;
  increment(name: string, cardinality: string, amount: number): void;
  entries(name: string): string;
}

function host(api: string): Host {
  const found = (globalThis as { __codemodV1Host?: Host }).__codemodV1Host;
  if (!found) {
    throw new Error(
      `${api} works only in a workflow.yaml js-ast-grep step (codemod 1.x compatibility)`,
    );
  }
  return found;
}

/** The state earlier steps left, as the workflow passed it for this file. */
function earlier(): Record<string, unknown> {
  return (globalThis as { __codemodV1State?: Record<string, unknown> }).__codemodV1State ?? {};
}

function copy<T>(value: T): T {
  return value === undefined ? value : (JSON.parse(JSON.stringify(value)) as T);
}

export function getState<T>(name: string): T | undefined {
  const found = host("getState").get(String(name));
  if (found !== undefined) {
    const entry = JSON.parse(found) as { value?: T; unset?: boolean };
    return entry.unset ? undefined : entry.value;
  }
  const state = earlier();
  return Object.prototype.hasOwnProperty.call(state, name) ? copy(state[name] as T) : undefined;
}

export function setState<T>(name: string, value: T, persist = true): void {
  const json = JSON.stringify(value);
  if (json === undefined) {
    host("setState").unset(String(name));
    return;
  }
  host("setState").set(String(name), json, persist !== false);
}

export function unsetState(name: string): void {
  host("unsetState").unset(String(name));
}

export function acquireLock(name: string): () => void {
  const shared = host("acquireLock");
  const key = String(name);
  shared.lock(key);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    shared.unlock(key);
  };
}

function removed(name: string): () => never {
  return () => {
    throw new Error(
      `${name} was removed, and codemod 1.x compatibility does not provide it: use getState and setState`,
    );
  };
}

export const setStepOutput = removed("setStepOutput");
export const getStepOutput = removed("getStepOutput");
export const getOrSetStepOutput = removed("getOrSetStepOutput");

type Cardinality = Record<string, string | undefined | null>;

export interface MetricEntry {
  cardinality: Record<string, string>;
  count: number;
}

export interface MetricAtom {
  readonly name: string;
  increment(cardinality?: Cardinality, amount?: number): void;
  getEntries(): MetricEntry[];
}

export function useMetricAtom(name: string): MetricAtom {
  const metric = String(name);
  return Object.freeze({
    name: metric,
    increment(cardinality: Cardinality = {}, amount = 1) {
      // Keys sorted, null and undefined left out, as 1.x did.
      const clean: Record<string, string> = {};
      for (const key of Object.keys(cardinality ?? {}).sort()) {
        const value = cardinality[key];
        if (value !== undefined && value !== null) clean[key] = String(value);
      }
      host("useMetricAtom").increment(metric, JSON.stringify(clean), Number(amount));
    },
    getEntries(): MetricEntry[] {
      return JSON.parse(host("useMetricAtom").entries(metric)) as MetricEntry[];
    },
  });
}

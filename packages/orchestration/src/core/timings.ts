/**
 * Benchmark phase timings: the TypeScript half of `crates/bench-timings`
 * (same phase names, same schema; see `benchmarks/runtimes/README.md`,
 * "Timings"). Off unless a host turns it on (`configureTimings`): the Node
 * CLI does when `CODEMOD_TIMINGS_FILE` is set (`host/timings-file.ts`), the
 * QuickJS bundle when the Rust host was built with its `timings` feature
 * and the variable is set.
 *
 * Off, `now()` and `lap()` return 0 and record nothing (one load of a
 * module-level boolean; V8 inlines them). On, spans add to `Float64Array`
 * accumulators; the clock is the host's nanosecond clock
 * (`performance.now()` on Node, `Host.clock.ns()` in QuickJS; never
 * `Date.now()`, whose resolution is 1 ms). One JS thread runs everything,
 * so spans are wall time on the critical path.
 *
 * Portable (`tests/portable.test.ts`): no Node API.
 */

/** The fixed phase names, in `crates/bench-timings` order. */
export const PHASE_NAMES = [
  "boot",
  "plan",
  "select",
  "read",
  "setup",
  "parse",
  "transform",
  "convert",
  "conflict",
  "commit",
  "idle_wait",
] as const;

export type PhaseName = (typeof PHASE_NAMES)[number];

/** Phase indices for `lap`/`add`. */
export const Phase = {
  boot: 0,
  plan: 1,
  select: 2,
  read: 3,
  setup: 4,
  parse: 5,
  transform: 6,
  convert: 7,
  conflict: 8,
  commit: 9,
  idle_wait: 10,
} as const satisfies Record<PhaseName, number>;

export type PhaseId = (typeof Phase)[PhaseName];

/** One phase in the schema: wall `ns`, span `count`, optional `cpu_ns`. */
export interface PhaseTotal {
  ns: number;
  count: number;
  cpu_ns?: number;
}

/** The schema's `phases` object. */
export type PhaseTotals = Partial<Record<PhaseName, PhaseTotal>>;

let ON = false;
let clock: () => number = () => 0;
let arm: string | undefined;
const wall = new Float64Array(PHASE_NAMES.length);
const count = new Float64Array(PHASE_NAMES.length);
const cpu = new Float64Array(PHASE_NAMES.length);
const children: number[] = [];

/**
 * Turn timings on or off for this realm. `clock` returns monotonic
 * nanoseconds (any origin: only differences are used).
 */
export function configureTimings(options: { enabled: boolean; clock?: () => number }): void {
  ON = options.enabled && options.clock !== undefined;
  clock = options.clock ?? (() => 0);
}

export function timingsEnabled(): boolean {
  return ON;
}

/** The clock in nanoseconds; 0 when off. */
export function now(): number {
  return ON ? clock() : 0;
}

/** End a span of `phase` started at `start` (from `now()`); returns now, the next span's start. */
export function lap(phase: PhaseId, start: number): number {
  if (!ON) return 0;
  const end = clock();
  wall[phase]! += end - start;
  count[phase]! += 1;
  return end;
}

/** Add `ns` of wall time in `spans` spans (and optionally CPU time) to `phase`. */
export function add(phase: PhaseId, ns: number, spans = 1, cpuNs = 0): void {
  if (!ON) return;
  wall[phase]! += ns;
  count[phase]! += spans;
  cpu[phase]! += cpuNs;
}

/** The arm name the top-level file reports (`ts-<runtime>`). */
export function setTimingsArm(name: string): void {
  arm = name;
}

export function timingsArm(): string | undefined {
  return arm;
}

/** A child process that writes its own timings file (`<path>.<pid>.json`). */
export function registerTimingsChild(pid: number | undefined): void {
  if (ON && pid !== undefined) children.push(pid);
}

export function timingsChildren(): readonly number[] {
  return children;
}

/**
 * Every phase so far: this realm's spans plus each source's, summed (they
 * are disjoint wall time on the one JS thread, or CPU time of workers).
 * Empty phases are omitted; nanoseconds are whole numbers.
 */
export function timingsSnapshot(): PhaseTotals {
  const totals: PhaseTotals = {};
  const merge = (name: PhaseName, ns: number, spans: number, cpuNs: number) => {
    if (ns === 0 && spans === 0 && cpuNs === 0) return;
    const total = (totals[name] ??= { ns: 0, count: 0 });
    total.ns += ns;
    total.count += spans;
    if (cpuNs > 0) total.cpu_ns = (total.cpu_ns ?? 0) + cpuNs;
  };
  PHASE_NAMES.forEach((name, index) => merge(name, wall[index]!, count[index]!, cpu[index]!));
  for (const total of Object.values(totals)) {
    total.ns = Math.round(total.ns);
    if (total.cpu_ns !== undefined) total.cpu_ns = Math.round(total.cpu_ns);
  }
  return totals;
}

/** Test hook: forget everything recorded. */
export function resetTimings(): void {
  wall.fill(0);
  count.fill(0);
  cpu.fill(0);
  children.length = 0;
  arm = undefined;
}

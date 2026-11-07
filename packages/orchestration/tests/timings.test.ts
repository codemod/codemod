/**
 * Benchmark timings: the portable accumulators (`core/timings.ts`) and the
 * Node CLI's child-process accounting (`host/timings-file.ts`). The files
 * the arms write end to end are checked by `benchmarks/runtimes`
 * (`overhead.sh`, `rounds.sh`) and `tests/timings.e2e.test.ts`.
 */
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  PHASE_NAMES,
  Phase,
  add,
  addTimingsSource,
  configureTimings,
  lap,
  now,
  registerTimingsChild,
  resetTimings,
  setTimingsArm,
  timingsArm,
  timingsChildren,
  timingsEnabled,
  timingsSnapshot,
} from "../src/core/timings.ts";
import { TIMINGS_FILE_ENV, timeChildProcess } from "../src/host/timings-file.ts";

/** A clock the test advances by hand. */
function fakeClock(start = 1_000) {
  let t = start;
  return {
    read: () => t,
    advance: (ns: number) => {
      t += ns;
    },
  };
}

afterEach(() => {
  configureTimings({ enabled: false });
  resetTimings();
});

describe("core/timings", () => {
  it("matches the Rust phase contract (crates/bench-timings PHASE_NAMES)", () => {
    expect(PHASE_NAMES).toEqual([
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
    ]);
    PHASE_NAMES.forEach((name, index) => expect(Phase[name]).toBe(index));
  });

  it("records nothing when off", () => {
    expect(timingsEnabled()).toBe(false);
    expect(now()).toBe(0);
    expect(lap(Phase.parse, 0)).toBe(0);
    add(Phase.transform, 5);
    addTimingsSource(() => ({ parse: { ns: 1, count: 1 } }));
    registerTimingsChild(42);
    expect(timingsSnapshot()).toEqual({});
    expect(timingsChildren()).toEqual([]);
  });

  it("needs a clock to turn on", () => {
    configureTimings({ enabled: true });
    expect(timingsEnabled()).toBe(false);
  });

  it("accumulates laps, spans, and CPU time per phase", () => {
    const clock = fakeClock();
    configureTimings({ enabled: true, clock: clock.read });
    let mark = now();
    clock.advance(10);
    mark = lap(Phase.select, mark);
    clock.advance(7);
    mark = lap(Phase.read, mark);
    clock.advance(3);
    lap(Phase.select, mark);
    add(Phase.parse, 100, 4, 250);
    expect(timingsSnapshot()).toEqual({
      select: { ns: 13, count: 2 },
      read: { ns: 7, count: 1 },
      parse: { ns: 100, count: 4, cpu_ns: 250 },
    });
  });

  it("sums native sources into the snapshot and rounds to whole nanoseconds", () => {
    configureTimings({ enabled: true, clock: () => 0 });
    add(Phase.parse, 10.4, 1);
    addTimingsSource(() => ({
      parse: { ns: 5, count: 2, cpu_ns: 30.6 },
      convert: { ns: 2, count: 1 },
    }));
    addTimingsSource(() => undefined);
    expect(timingsSnapshot()).toEqual({
      parse: { ns: 15, count: 3, cpu_ns: 31 },
      convert: { ns: 2, count: 1 },
    });
  });

  it("keeps the arm and children until reset", () => {
    configureTimings({ enabled: true, clock: () => 0 });
    setTimingsArm("ts-node");
    registerTimingsChild(7);
    registerTimingsChild(undefined);
    expect(timingsArm()).toBe("ts-node");
    expect(timingsChildren()).toEqual([7]);
    resetTimings();
    expect(timingsArm()).toBeUndefined();
    expect(timingsChildren()).toEqual([]);
  });
});

describe("host/timings-file timeChildProcess", () => {
  const saved = process.env[TIMINGS_FILE_ENV];
  let dir: string | undefined;
  afterEach(() => {
    if (saved === undefined) delete process.env[TIMINGS_FILE_ENV];
    else process.env[TIMINGS_FILE_ENV] = saved;
    if (dir !== undefined) rmSync(dir, { recursive: true, force: true });
    dir = undefined;
  });

  it("does nothing when off", () => {
    const timed = timeChildProcess();
    expect(timed.env).toEqual({});
    timed.exited(123);
    expect(timingsChildren()).toEqual([]);
  });

  it("charges spawn-to-close minus the child's wall to setup and registers the child", () => {
    dir = mkdtempSync(join(tmpdir(), "codemod-timings-"));
    const path = join(dir, "run.timings.json");
    process.env[TIMINGS_FILE_ENV] = path;
    const clock = fakeClock();
    configureTimings({ enabled: true, clock: clock.read });

    const timed = timeChildProcess();
    expect(Number(timed.env.BENCH_T0_NS)).toBeGreaterThan(0);
    clock.advance(1_000);
    writeFileSync(`${path}.4242.json`, JSON.stringify({ schema: 1, wall_ns: 700 }));
    timed.exited(4242);

    expect(timingsChildren()).toEqual([4242]);
    expect(timingsSnapshot()).toEqual({ setup: { ns: 300, count: 1 } });
  });

  it("charges the whole span when the child wrote no file", () => {
    dir = mkdtempSync(join(tmpdir(), "codemod-timings-"));
    process.env[TIMINGS_FILE_ENV] = join(dir, "run.timings.json");
    const clock = fakeClock();
    configureTimings({ enabled: true, clock: clock.read });

    const timed = timeChildProcess();
    clock.advance(500);
    timed.exited(99);
    expect(timingsSnapshot()).toEqual({ setup: { ns: 500, count: 1 } });
  });
});

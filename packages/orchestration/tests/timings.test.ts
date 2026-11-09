/**
 * Benchmark timings: the portable accumulators (`core/timings.ts`). The
 * files the arms write end to end are checked by `benchmarks/runtimes`
 * (`overhead.sh`, `rounds.sh`) and `crates/cli/tests/typescript_timings.rs`.
 */
import { afterEach, describe, expect, it } from "vitest";
import {
  PHASE_NAMES,
  Phase,
  add,
  configureTimings,
  lap,
  now,
  resetTimings,
  timingsEnabled,
  timingsSnapshot,
} from "../src/core/timings.ts";

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
    expect(timingsSnapshot()).toEqual({});
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

  it("rounds the snapshot to whole nanoseconds", () => {
    configureTimings({ enabled: true, clock: () => 0 });
    add(Phase.parse, 10.4, 1, 30.6);
    expect(timingsSnapshot()).toEqual({ parse: { ns: 10, count: 1, cpu_ns: 31 } });
  });
});

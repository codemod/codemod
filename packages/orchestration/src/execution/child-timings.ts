/**
 * Benchmark timings for child processes the Node CLI waits on (the bridge,
 * the quickjs host), each of which writes its own timings file
 * (`<CODEMOD_TIMINGS_FILE>.<pid>.json`, `crates/bench-timings`). Node-only;
 * the accumulators are `core/timings.ts`, the CLI's own file
 * `host/timings-file.ts`.
 */
import { readFileSync } from "node:fs";
import { Phase, add, now, registerTimingsChild, timingsEnabled } from "../core/timings.ts";

export const TIMINGS_FILE_ENV = "CODEMOD_TIMINGS_FILE";
export const BENCH_T0_ENV = "BENCH_T0_NS";

/**
 * Time a child process that writes its own timings file. Call right before
 * `spawn`; returns the extra environment for the child (`BENCH_T0_NS` at
 * spawn, so its `exec_to_main_ns` reports its own exec) and `exited(pid)` for
 * the child's `close`. `exited` registers the child and adds to this
 * process's `setup` the part of spawn-to-close the child's own `wall_ns`
 * does not cover (fork/exec, loader, runtime start, exit, pipe close), so
 * merged phases add up without double counting the child's phases. Both
 * are no-ops when timings are off.
 */
export function timeChildProcess(): {
  env: Record<string, string>;
  exited: (pid: number | undefined) => void;
} {
  if (!timingsEnabled()) return { env: {}, exited: () => {} };
  const spawned = now();
  return {
    env: { [BENCH_T0_ENV]: String(Number(process.hrtime.bigint())) },
    exited: (pid) => {
      const span = now() - spawned;
      registerTimingsChild(pid);
      const path = process.env[TIMINGS_FILE_ENV];
      if (pid === undefined || path === undefined) return;
      let childWall = 0;
      try {
        const document = JSON.parse(readFileSync(`${path}.${pid}.json`, "utf8")) as {
          wall_ns?: number;
        };
        childWall = document.wall_ns ?? 0;
      } catch {
        // No file (e.g. a child built without timings): the whole span is overhead.
      }
      add(Phase.setup, Math.max(0, span - childWall));
    },
  };
}

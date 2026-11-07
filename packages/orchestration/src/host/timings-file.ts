/**
 * The Node CLI's benchmark timings file (Node-only; the portable
 * accumulators are `core/timings.ts`). With `CODEMOD_TIMINGS_FILE=<path>`
 * set, `installTimingsFile()` turns timings on, records `boot` (Node start
 * until the CLI's `main`), and writes one schema-1 document at process exit:
 * to `<path>` when this is the top-level process, else `<path>.<pid>.json`.
 * It sets `CODEMOD_TIMINGS_ROOT_PID` for child processes, which therefore
 * write `<path>.<pid>.json`; the children registered with
 * `registerTimingsChild` whose files exist are listed in `children`.
 *
 * Main start is Node's `performance.timeOrigin` (process start), on the
 * monotonic raw clock of `process.hrtime` (`CLOCK_UPTIME_RAW` on macOS, as
 * `BENCH_T0_NS` from `bench-exec`).
 */
import { existsSync, writeFileSync } from "node:fs";
import {
  Phase,
  add,
  configureTimings,
  timingsArm,
  timingsChildren,
  timingsSnapshot,
} from "../core/timings.ts";

export const TIMINGS_FILE_ENV = "CODEMOD_TIMINGS_FILE";
export const TIMINGS_ROOT_PID_ENV = "CODEMOD_TIMINGS_ROOT_PID";

let installed = false;

/** Turn timings on when `CODEMOD_TIMINGS_FILE` is set; returns whether they are on. */
export function installTimingsFile(env: NodeJS.ProcessEnv = process.env): boolean {
  const path = env[TIMINGS_FILE_ENV];
  if (path === undefined || path === "") return false;
  if (installed) return true;
  installed = true;
  const bootNs = performance.now() * 1e6;
  // The hrtime reading of `performance.timeOrigin`.
  const mainStartNs = Number(process.hrtime.bigint()) - bootNs;
  configureTimings({ enabled: true, clock: () => performance.now() * 1e6 });
  add(Phase.boot, bootNs);
  const root = env[TIMINGS_ROOT_PID_ENV];
  const topLevel = root === undefined || root.trim() === String(process.pid);
  if (root === undefined) env[TIMINGS_ROOT_PID_ENV] = String(process.pid);
  const file = topLevel ? path : `${path}.${process.pid}.json`;
  process.once("exit", () => {
    const t0 = Number(env.BENCH_T0_NS);
    const document = {
      schema: 1,
      arm: timingsArm() ?? "ts",
      pid: process.pid,
      parent_pid: process.ppid ?? null,
      exec_to_main_ns: Number.isFinite(t0) && t0 > 0 ? Math.round(mainStartNs - t0) : null,
      wall_ns: Math.round(performance.now() * 1e6),
      phases: timingsSnapshot(),
      marks: {},
      children: timingsChildren()
        .map((pid) => `${path}.${pid}.json`)
        .filter((child) => existsSync(child)),
    };
    try {
      writeFileSync(file, `${JSON.stringify(document)}\n`);
    } catch {
      // Best effort: timings never fail the run.
    }
  });
  return true;
}

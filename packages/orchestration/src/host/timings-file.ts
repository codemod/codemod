/**
 * The Node CLI's benchmark timings file (Node-only; the portable
 * accumulators are `core/timings.ts`). With `CODEMOD_TIMINGS_FILE=<path>`
 * set, `installTimingsFile()` turns timings on and writes one schema-1
 * document at process exit (`endBoot()` records `boot`, Node start until the
 * CLI's `main`):
 * to `<path>` when this is the top-level process, else `<path>.<pid>.json`.
 * It sets `CODEMOD_TIMINGS_ROOT_PID` for child processes, which therefore
 * write `<path>.<pid>.json`; the children registered with
 * `registerTimingsChild` whose files exist are listed in `children`.
 *
 * Main start is Node's `performance.timeOrigin` (process start), on the
 * monotonic clock of `process.hrtime` (`CLOCK_UPTIME_RAW` on macOS,
 * `CLOCK_MONOTONIC` on Linux), as `BENCH_T0_NS` from `bench-exec`. Child
 * processes are timed by `execution/child-timings.ts`.
 */
import { existsSync, writeFileSync } from "node:fs";
import {
  Phase,
  add,
  configureTimings,
  now,
  timingsArm,
  timingsChildren,
  timingsEnabled,
  timingsSnapshot,
} from "../core/timings.ts";

import { TIMINGS_FILE_ENV } from "../execution/child-timings.ts";

export { TIMINGS_FILE_ENV, timeChildProcess } from "../execution/child-timings.ts";
export const TIMINGS_ROOT_PID_ENV = "CODEMOD_TIMINGS_ROOT_PID";

let installed = false;

let booted = false;

/** Record `boot`: process start until now (the CLI's `main`); once. */
export function endBoot(): void {
  if (!timingsEnabled() || booted) return;
  booted = true;
  add(Phase.boot, now());
}

/** Turn timings on when `CODEMOD_TIMINGS_FILE` is set; returns whether they are on. */
export function installTimingsFile(env: NodeJS.ProcessEnv = process.env): boolean {
  const path = env[TIMINGS_FILE_ENV];
  if (path === undefined || path === "") return false;
  if (installed) return true;
  installed = true;
  // The hrtime reading of `performance.timeOrigin`.
  const mainStartNs = Number(process.hrtime.bigint()) - performance.now() * 1e6;
  configureTimings({ enabled: true, clock: () => performance.now() * 1e6 });
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

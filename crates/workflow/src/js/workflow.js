// The `codemod:workflow` module of the engine's QuickJS context: the shared
// orchestrator (`orchestrator.js`) bound to the host the engine installs as
// `globalThis.__workflowHost` before this module evaluates:
//   history(): string                    steps recorded by earlier runs, as JSON
//   record(entry: string, ms: number)    persist one step's outcome
//   event(event: string)                 report step started / replayed
//   canceled(): boolean                  whether the run was asked to stop
//   start(request: string): id           start a host task; settled via __workflowSettle
//   evaluate(kind, expression, scope)    evaluate a workflow.yaml `${{ }}` expression
//   semver(op, ecosystem, a, b): string   compare versions and ranges, as JSON
//   now(): number                        the real time, for step records
//   advanceClock(ms: number)             move the body's clock (Date.now()) forward
//   testPlan(): string                   `{ until, given }` when the run is a test, else "null"
//   stop()                               the test's `until` step has started

import { createRuntime } from "codemod:workflow/orchestrator";
import { makeSemver } from "codemod:workflow/semver";

export {
  AwaitingApprovalError,
  CanceledError,
  NondeterminismError,
  StepError,
} from "codemod:workflow/orchestrator";

const raw = globalThis.__workflowHost;
const pending = new Map();

const runtime = createRuntime({
  history: () => JSON.parse(raw.history()),
  record: (entry, ms) => raw.record(JSON.stringify(entry), ms),
  event: (event) => raw.event(JSON.stringify(event)),
  canceled: () => raw.canceled(),
  now: () => raw.now(),
  advanceClock: (ms) => raw.advanceClock(ms),
  testPlan: () => JSON.parse(raw.testPlan()),
  stop: () => raw.stop(),
  evaluate: (kind, expression, scope) =>
    JSON.parse(raw.evaluate(kind, expression, JSON.stringify(scope))),
  start({ position, label, task, state }) {
    const id = raw.start(JSON.stringify({ position, label, task, state }));
    return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
  },
});

globalThis.__workflowSettle = (id, ok, payload) => {
  const entry = pending.get(id);
  pending.delete(id);
  if (!entry) return;
  if (ok) {
    entry.resolve(JSON.parse(payload));
    return;
  }
  try {
    entry.reject(JSON.parse(payload));
  } catch {
    entry.reject({ name: "Error", message: payload });
  }
};

export const {
  getState,
  setState,
  unsetState,
  __callback,
  __evaluate,
  forEachFile,
  dependencyGraph,
  shell,
  jssg,
  astGrep,
  shard,
  codemod,
  agent,
  installSkill,
  approval,
  branch,
  commit,
  pullRequest,
  step,
} = runtime;

// `registry` and `semver` in the body. Inside steps and callbacks the build
// imports them from `codemod:workflow/dependencies` instead, which runs in
// the isolate.

/** Package releases from npm and crates.io; only inside a step or callback, so the answer is recorded. */
export const registry = Object.freeze({
  npm: () => outsideStep("npm"),
  cargo: () => outsideStep("cargo"),
});

function outsideStep(ecosystem) {
  return Promise.reject(
    new Error(`registry.${ecosystem}(...) must run inside a step, so its answer is recorded`),
  );
}

/** Versions and ranges, by npm rules or (with `{ ecosystem: "cargo" }`) Cargo's. */
export const semver = makeSemver((op, ecosystem, a, b) =>
  JSON.parse(raw.semver(op, ecosystem, a, b)),
);

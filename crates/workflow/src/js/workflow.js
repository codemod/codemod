// The `codemod/workflow` module of the engine's QuickJS context: the shared
// orchestrator (`orchestrator.js`) bound to the host the engine installs as
// `globalThis.__workflowHost` before this module evaluates:
//   history(): string                    steps recorded by earlier runs, as JSON
//   record(entry: string, ms: number)    persist one step's outcome
//   event(event: string)                 report step started / replayed
//   canceled(): boolean                  whether a step about to start must not
//   start(request: string): id           start a host task; settled via __workflowSettle
//   evaluate(kind, expression, scope)    evaluate a workflow.yaml `${{ }}` expression
//   now(): number                        the real time, for step records
//   advanceClock(ms: number)             move the body's clock (Date.now()) forward
//   testPlan(): string                   `{ until, given }` when the run is a test, else "null"
//   stop()                               the test's `until` step has started

import { createRuntime } from "codemod-internal:orchestrator";

export {
  AwaitingApprovalError,
  CanceledError,
  NondeterminismError,
  StepError,
} from "codemod-internal:orchestrator";

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

// The engine's loop settles replayed steps and closes the run through these.
globalThis.__workflowReplayNext = runtime.replayNext;
globalThis.__workflowClose = runtime.close;

export const {
  getState,
  setState,
  unsetState,
  acquireLock,
  setStepOutput,
  getStepOutput,
  getOrSetStepOutput,
  __callback,
  __evaluate,
  forEachFile,
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

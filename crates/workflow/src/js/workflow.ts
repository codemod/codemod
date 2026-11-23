// The `codemod/workflow` module of the engine's QuickJS context: the shared
// orchestrator (`orchestrator.ts`) bound to the host the engine installs as
// `globalThis.__workflowHost` before this module evaluates (`RawHost` in
// `globals.d.ts`). Everything crosses to Rust as JSON text.

import { createRuntime } from "codemod-internal:orchestrator";
import type { Outcome } from "codemod-internal:orchestrator";

export { AwaitingApprovalError, CanceledError, StepError } from "codemod-internal:orchestrator";

const raw = globalThis.__workflowHost;

/** Tasks the engine is running, by the id it gave them. */
const pending = new Map<
  number,
  { resolve(outcome: Outcome): void; reject(error: { name: string; message: string }): void }
>();

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
  // Callbacks are extracted by the build here, so no request carries a function.
  start({ fn: _, ...request }) {
    const id = raw.start(JSON.stringify(request));
    return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
  },
});

// The engine's loop settles tasks, settles replayed steps and closes the run
// through these.
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
globalThis.__workflowReplayNext = runtime.replayNext;
globalThis.__workflowClose = runtime.close;

export const {
  __callback,
  __importedCallback,
  __evaluate,
  // `codemod/deps` exports it as `dependencyGraph`.
  dependencyGraph: __dependencyGraph,
  forEachFile,
  selectFiles,
  shell,
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
  sleep,
  waitForEvent,
} = runtime;

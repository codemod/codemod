// Globals of a step's or callback's isolate, installed before its code runs.
//
// Callbacks are pure: what they do to the world goes through declared steps,
// which the run records and replays. So the network, timers and changes to
// the environment fail here with the step to use instead. `fs` is read-only
// and limited to the target (the engine installs it that way), and
// `child_process` is a module of such errors (`child_process.ts`).

import { unavailable } from "codemod-internal:orchestrator";

(() => {
  const globals = globalThis as unknown as Record<string, unknown>;
  const refuse = (name: string) =>
    function () {
      throw new Error(unavailable(name, "callback"));
    };
  for (const name of [
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "setTimeout",
    "setInterval",
    "setImmediate",
  ]) {
    globals[name] = refuse(name);
  }

  // No ambient host environment in this isolate. Keep a compatibility object
  // for reads and give writes the same diagnostic as the other effect guards.
  const env = new Proxy(Object.freeze(Object.create(null)), {
    set: refuse("process.env"),
    deleteProperty: refuse("process.env"),
    defineProperty: refuse("process.env"),
  });
  Object.defineProperty(globals, "process", { value: Object.freeze({ env }) });
})();

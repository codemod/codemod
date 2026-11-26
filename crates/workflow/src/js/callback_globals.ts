// Globals of a step's or callback's isolate, installed before its code runs.
//
// Callbacks are pure: what they do to the world goes through declared steps,
// which the run records and replays. So the network, timers and changes to
// the environment fail here with the step to use instead. `codemod/files`
// is limited to the target, and
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

  const process = globals.process as { env?: Record<string, string> } | undefined;
  if (process && typeof process.env === "object" && process.env !== null) {
    const env = process.env;
    const readOnly = new Proxy(env, {
      set() {
        throw new Error(unavailable("process.env", "callback"));
      },
      deleteProperty() {
        throw new Error(unavailable("process.env", "callback"));
      },
      defineProperty() {
        throw new Error(unavailable("process.env", "callback"));
      },
    });
    try {
      Object.defineProperty(process, "env", { value: readOnly, writable: false });
    } catch {
      // A process object that keeps its env: leave it.
    }
  }
})();

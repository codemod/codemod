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

  // No ambient host environment in this isolate, and no native `process`
  // (the engine withholds the module): an inert one with what transforms
  // read that is no secret. `env` is empty, and writes to it fail like the
  // other effects.
  const host = (globals.__codemodHostProcess ?? {}) as {
    cwd?: string;
    argv?: string[];
    platform?: string;
    arch?: string;
  };
  delete globals.__codemodHostProcess;
  const env = new Proxy(Object.freeze(Object.create(null)), {
    set: refuse("process.env"),
    deleteProperty: refuse("process.env"),
    defineProperty: refuse("process.env"),
  });
  const write = (text: unknown) => {
    const line = String(text).replace(/\n$/, "");
    if (line) (globals.console as { log(line: string): void }).log(line);
    return true;
  };
  const stream = Object.freeze({ write, isTTY: false });
  const process = {
    env,
    argv: Object.freeze([...(host.argv ?? [])]),
    argv0: host.argv?.[0] ?? "codemod",
    execArgv: Object.freeze([]),
    platform: host.platform ?? "linux",
    arch: host.arch ?? "x64",
    version: "v22.0.0",
    versions: Object.freeze({ node: "22.0.0" }),
    pid: 0,
    exitCode: undefined as number | undefined,
    stdout: stream,
    stderr: stream,
    cwd: () => host.cwd ?? "/",
    exit: refuse("process.exit"),
    nextTick: (callback: (...args: unknown[]) => void, ...args: unknown[]) => {
      void Promise.resolve().then(() => callback(...args));
    },
    hrtime: Object.assign(() => [Math.floor(Date.now() / 1000), 0], {
      bigint: () => BigInt(Date.now()) * 1000000n,
    }),
    emitWarning: () => {},
    on: () => process,
    off: () => process,
    once: () => process,
  };
  Object.defineProperty(globals, "process", { value: process });
})();

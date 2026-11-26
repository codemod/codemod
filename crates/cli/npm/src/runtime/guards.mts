// What workflow code may use when Node.js or Bun runs it, as in the CLI.
//
// Workflow callbacks (function steps, `forEachFile` callbacks and reducers)
// are pure: they get the file API, reading files, and the analysis APIs.
// They cannot reach the network, run processes, write files other than
// through the file API, change `process.env`, or start timers: those are
// declared steps (`http()`, `shell()`, `sleep()`), recorded and not done
// again on resume. The workflow body cannot either, and its `Date`,
// `performance.now`, `Math.random`, `crypto.getRandomValues` and
// `crypto.randomUUID` are replayed: a workflow clock and numbers seeded by
// the run, as crates/workflow/src/js/body_globals.ts gives the CLI's body.
//
// The globals and built-in modules are wrapped once per process. A wrapper
// only changes anything for code that runs inside a run (the run's
// AsyncLocalStorage scope is set): the test file itself, and anything else
// outside a run, keeps what Node.js gives. The host's own code runs inside
// the run's scope too, so it uses `real`, captured before any wrapping.

import { createRequire, syncBuiltinESMExports } from "node:module";

import { context } from "./context.mts";
import type { RunScope } from "./context.mts";
import { unavailable } from "./orchestrator.mts";

const require = createRequire(import.meta.url);

type AnyFunction = (...args: unknown[]) => unknown;

/** The real globals, from before any wrapping. */
export interface RealGlobals {
  setTimeout: typeof setTimeout;
  clearTimeout: typeof clearTimeout;
  setImmediate: typeof setImmediate;
  dateNow: () => number;
  performanceNow: () => number;
  random: () => number;
}

interface Installed {
  real: RealGlobals;
}

const KEY = Symbol.for("codemod.host.guards");
const shared = globalThis as { [KEY]?: Installed };

/** Install the guards (once per process) and give the real globals. */
export function installGuards(): RealGlobals {
  const installed = shared[KEY];
  if (installed) return installed.real;
  const real: RealGlobals = {
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
    setImmediate: globalThis.setImmediate,
    dateNow: Date.now.bind(Date),
    performanceNow: performance.now.bind(performance),
    random: Math.random,
  };
  shared[KEY] = { real };
  install(real);
  return real;
}

/** The run the calling code belongs to, if any, and where in it. */
function scope(): { store: RunScope; site: "body" | "callback" } | undefined {
  const store = context.getStore();
  return store ? { store, site: store.step ? "callback" : "body" } : undefined;
}

/** Throws when the calling code runs inside a run. */
function refuse(name: string): void {
  const inside = scope();
  if (inside) throw new Error(unavailable(name, inside.site));
}

/** The body's clock, when the calling code is a run's body. */
function bodyClock(): RunScope["body"] {
  const store = context.getStore();
  return store && !store.step ? store.body : undefined;
}

/** `original`, refused inside a run. */
function guarded<F extends AnyFunction>(name: string, original: F): F {
  const wrapper = function (this: unknown, ...args: unknown[]) {
    refuse(name);
    return original.apply(this, args);
  };
  Object.defineProperty(wrapper, "name", { value: original.name });
  return wrapper as F;
}

/** Wrap `object[key]` when it is a function. */
function wrapMethod(
  object: Record<string, unknown>,
  key: string,
  wrap: (original: AnyFunction) => AnyFunction,
): void {
  const original = object[key];
  if (typeof original !== "function") return;
  try {
    object[key] = wrap(original as AnyFunction);
  } catch {
    // read-only in this runtime: left as it is
  }
}

/** A constructor (`XMLHttpRequest`, `WebSocket`), refused inside a run. */
function guardedClass(name: string, original: object): object {
  return new Proxy(original, {
    construct(target, args, newTarget) {
      refuse(name);
      return Reflect.construct(target as new (...a: unknown[]) => object, args, newTarget);
    },
    apply(target, self, args) {
      refuse(name);
      return Reflect.apply(target as AnyFunction, self, args);
    },
  });
}

const CHILD_PROCESS = [
  "exec",
  "execSync",
  "execFile",
  "execFileSync",
  "spawn",
  "spawnSync",
  "fork",
] as const;

function install(real: RealGlobals): void {
  const globals = globalThis as unknown as Record<string, unknown>;

  // Network: `http()` steps.
  wrapMethod(globals, "fetch", (original) => guarded("fetch", original));
  for (const name of ["XMLHttpRequest", "WebSocket"]) {
    const original = globals[name];
    if (typeof original === "function") globals[name] = guardedClass(name, original);
  }

  // Timers: `sleep()` between steps. Also in `node:timers`.
  const timers = require("node:timers") as Record<string, unknown>;
  for (const name of ["setTimeout", "setInterval", "setImmediate"]) {
    const original = globals[name];
    if (typeof original !== "function") continue;
    const wrapper = guarded(name, original as AnyFunction);
    globals[name] = wrapper;
    if (timers[name] === original) timers[name] = wrapper;
  }
  try {
    const promises = require("node:timers/promises") as Record<string, unknown>;
    for (const name of ["setTimeout", "setInterval", "setImmediate"]) {
      wrapMethod(promises, name, (original) => guarded(name, original));
    }
  } catch {
    // no timers/promises in this runtime
  }

  // Processes: `shell()` steps.
  const childProcess = require("node:child_process") as Record<string, unknown>;
  for (const name of CHILD_PROCESS) {
    wrapMethod(childProcess, name, (original) => guarded(`child_process.${name}`, original));
  }

  // File access: `codemod/files`, even for workflows supplied as preloaded modules.
  const fs = require("node:fs") as Record<string, unknown>;
  const fsPromises = fs.promises as Record<string, unknown>;
  for (const [object, module] of [
    [fs, "fs"],
    [fsPromises, "fs/promises"],
  ] as const) {
    for (const name of Object.keys(object)) {
      // Node's loader itself uses the `.native` realpath entry point.
      if (name === "realpathSync") {
        const original = object[name] as typeof import("node:fs").realpathSync;
        object[name] = Object.assign(guarded(`${module}.${name}`, original as AnyFunction), {
          native: original.native,
        });
      } else {
        wrapMethod(object, name, (original) => guarded(`${module}.${name}`, original));
      }
    }
  }

  // ESM named imports of the built-in modules see the wrappers too.
  if (typeof syncBuiltinESMExports === "function") syncBuiltinESMExports();

  // `process.env`: read, not changed (a step takes its variables).
  const env = process.env;
  const refuseEnv = () => refuse("process.env");
  try {
    process.env = new Proxy(env, {
      get: (target, key) => target[key as string],
      set(target, key, value) {
        refuseEnv();
        target[key as string] = value as string;
        return true;
      },
      deleteProperty(target, key) {
        refuseEnv();
        return delete target[key as string];
      },
      defineProperty(target, key, descriptor) {
        refuseEnv();
        return Reflect.defineProperty(target, key, descriptor);
      },
    });
  } catch {
    // process.env cannot be replaced in this runtime
  }

  // The body's clock and random numbers.
  const RealDate = Date;
  const now = () => bodyClock()?.now() ?? real.dateNow();
  globals.Date = new Proxy(RealDate, {
    construct(target, args, newTarget) {
      const clock = args.length === 0 ? bodyClock() : undefined;
      return Reflect.construct(target, clock ? [clock.now()] : args, newTarget);
    },
    apply(target, self, args) {
      const clock = bodyClock();
      return clock ? new RealDate(clock.now()).toString() : Reflect.apply(target, self, args);
    },
    get(target, key, receiver) {
      if (key === "now") return now;
      return Reflect.get(target, key, receiver) as unknown;
    },
  });
  performance.now = function now() {
    const clock = bodyClock();
    return clock ? clock.now() - clock.origin : real.performanceNow();
  };
  Math.random = function random() {
    return bodyClock()?.random() ?? real.random();
  };

  const webCrypto = globals.crypto as Record<string, unknown> | undefined;
  if (webCrypto) {
    wrapMethod(
      webCrypto,
      "getRandomValues",
      (original) =>
        function getRandomValues(this: unknown, array: unknown) {
          const clock = bodyClock();
          return clock
            ? seededValues(clock.random, array as Uint8Array)
            : original.call(webCrypto, array);
        },
    );
    wrapMethod(
      webCrypto,
      "randomUUID",
      (original) =>
        function randomUUID() {
          const clock = bodyClock();
          return clock ? seededUuid(clock.random) : original.call(webCrypto);
        },
    );
  }
}

/** `array` filled from `random`, as body_globals.ts does. */
function seededValues<T extends { length: number; BYTES_PER_ELEMENT?: number }>(
  random: () => number,
  array: T,
): T {
  const range = 2 ** (8 * (array.BYTES_PER_ELEMENT ?? 1));
  const items = array as unknown as number[];
  for (let index = 0; index < array.length; index++) {
    items[index] = Math.floor(random() * range);
  }
  return array;
}

function seededUuid(random: () => number): string {
  const bytes = seededValues(random, new Uint8Array(16));
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/**
 * The body's clock and random numbers for a run: as the CLI gives its body
 * (crates/workflow/src/run.rs: the clock starts at the run's start time, the
 * seed is the first 4 bytes, little-endian, of sha256(run id)).
 */
export function bodyClockFor(startedAt: number, seed: number): NonNullable<RunScope["body"]> {
  let clock = startedAt;
  let state = seed >>> 0 || 0x9e3779b9;
  return {
    origin: startedAt,
    now: () => clock,
    advance(ms) {
      if (ms > clock) clock = ms;
    },
    // mulberry32, as body_globals.ts
    random() {
      state = (state + 0x6d2b79f5) >>> 0;
      let t = state;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    },
  };
}

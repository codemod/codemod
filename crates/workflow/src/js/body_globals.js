// Globals of the workflow body, installed before any module runs.
//
// The body runs again on every resume, and each run must issue the same
// steps, so anything that gives a different answer each time is made
// replay-safe here:
//
// - Math.random, crypto.getRandomValues and crypto.randomUUID draw from a
//   generator seeded by the run id;
// - Date and performance.now read the workflow clock, which starts at the
//   run's start time and only moves when a step finishes, to the time
//   recorded for it.
//
// Inside steps and forEachFile callbacks (their own isolates) these are the
// real thing.
(() => {
  const host = globalThis.__workflowHost;

  let seed = host.seed() >>> 0 || 0x9e3779b9;
  // mulberry32
  const random = () => {
    seed = (seed + 0x6d2b79f5) >>> 0;
    let t = seed;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  Math.random = random;

  const RealDate = Date;
  const now = () => host.clock();
  function WorkflowDate(...args) {
    if (!new.target) return new RealDate(now()).toString();
    return args.length === 0 ? new RealDate(now()) : new RealDate(...args);
  }
  WorkflowDate.prototype = RealDate.prototype;
  WorkflowDate.now = now;
  WorkflowDate.parse = RealDate.parse;
  WorkflowDate.UTC = RealDate.UTC;
  globalThis.Date = WorkflowDate;

  const origin = now();
  globalThis.performance = Object.freeze({ timeOrigin: origin, now: () => now() - origin });

  const getRandomValues = (array) => {
    const range = 2 ** (8 * (array.BYTES_PER_ELEMENT ?? 1));
    for (let index = 0; index < array.length; index++) {
      array[index] = Math.floor(random() * range);
    }
    return array;
  };
  const randomUUID = () => {
    const bytes = getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  };
  globalThis.crypto = Object.freeze({ getRandomValues, randomUUID });

  const insideAStep = (name) =>
    function () {
      throw new Error(
        `${name} is not available in the workflow body, which runs again on every resume; use it inside a step or forEachFile callback`,
      );
    };
  for (const name of ["setTimeout", "setInterval", "setImmediate", "fetch"]) {
    if (!(name in globalThis)) globalThis[name] = insideAStep(name);
  }

  const format = (value) => {
    if (typeof value === "string") return value;
    if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
    try {
      return JSON.stringify(value) ?? String(value);
    } catch {
      return String(value);
    }
  };
  const log = (...items) => host.log(items.map(format).join(" "));
  globalThis.console = Object.freeze({ log, info: log, debug: log, warn: log, error: log });
})();

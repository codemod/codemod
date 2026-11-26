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
// Inside steps and forEachFile callbacks (their own isolates) the clock and
// randomness are the real thing: what a callback returns is recorded. The
// network and timers fail there too (callback_globals.ts).
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

  // What is replaced below is not in the types this file is checked with
  // (plain ECMAScript, no DOM and no Node.js), or is replaced by something
  // smaller than its type, so it is set through this view of the globals.
  const globals = globalThis as unknown as Record<string, unknown>;

  const RealDate = Date;
  const now = () => host.clock();
  // Called with `new` it makes a date; called without, a string, as Date does.
  function WorkflowDate(...args: unknown[]) {
    if (!new.target) return new RealDate(now()).toString();
    return args.length === 0
      ? new RealDate(now())
      : new RealDate(...(args as ConstructorParameters<DateConstructor>));
  }
  WorkflowDate.prototype = RealDate.prototype;
  WorkflowDate.now = now;
  WorkflowDate.parse = RealDate.parse;
  WorkflowDate.UTC = RealDate.UTC;
  globals.Date = WorkflowDate;

  const origin = now();
  globals.performance = Object.freeze({ timeOrigin: origin, now: () => now() - origin });

  const getRandomValues = <T extends Uint8Array | Uint16Array | Uint32Array>(array: T): T => {
    const range = 2 ** (8 * (array.BYTES_PER_ELEMENT ?? 1));
    for (let index = 0; index < array.length; index++) {
      array[index] = Math.floor(random() * range);
    }
    return array;
  };
  const randomUUID = () => {
    const bytes = getRandomValues(new Uint8Array(16));
    bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
    bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
    const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  };
  globals.crypto = Object.freeze({ getRandomValues, randomUUID });

  // The words of `unavailable(name, "body")` in orchestrator.ts (this file is
  // a script, so it cannot import it).
  const where = "the workflow body, which runs again on every resume";
  const refuse = (name: string, instead: string) =>
    function () {
      throw new Error(`${name} is not available in ${where}: ${instead}`);
    };
  for (const name of ["setTimeout", "setInterval", "setImmediate"]) {
    if (!(name in globals)) {
      globals[name] = refuse(name, 'wait between steps with await sleep("10s")');
    }
  }
  for (const name of ["fetch", "XMLHttpRequest", "WebSocket"]) {
    if (!(name in globals)) {
      globals[name] = refuse(
        name,
        'declare the request as a step, await step("label", http(url, options)), so it is recorded and not sent again on resume',
      );
    }
  }

  const format = (value: unknown) => {
    if (typeof value === "string") return value;
    if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
    try {
      return JSON.stringify(value) ?? String(value);
    } catch {
      return String(value);
    }
  };
  const log = (...items: unknown[]) => host.log(items.map(format).join(" "));
  globals.console = Object.freeze({ log, info: log, debug: log, warn: log, error: log });
})();

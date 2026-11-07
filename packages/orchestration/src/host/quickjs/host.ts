/**
 * The QuickJS `Host` (`core/host.ts`) over the Rust host's primitives
 * (`native.ts`). Runs inside QuickJS: Node-free (`tests/portable.test.ts`).
 */
import type { Host, HostChild, HostSelectRequest, HostSpawnOptions } from "../../core/host.ts";
import { createPosixPath } from "../../core/posix.ts";
import { createPropagatingAsyncContext } from "./async-context.ts";
import type { QuickjsNative } from "./native.ts";

type Timers = {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(timer: unknown): void;
};

function spawn(native: QuickjsNative, options: HostSpawnOptions): HostChild {
  const child = native.spawn(
    options.file,
    [...options.args],
    options.cwd,
    { ...options.env },
    {
      ...(options.stdin === undefined ? {} : { stdin: options.stdin }),
      stderr: options.stderr ?? "pipe",
      ...(options.onOutput === undefined ? {} : { onOutput: options.onOutput }),
    },
  );
  const signal = options.signal;
  if (signal === undefined) return { pid: child.pid, result: child.result };
  const onAbort = () => native.kill(child.id);
  if (signal.aborted) onAbort();
  else signal.addEventListener("abort", onAbort, { once: true });
  const settle = () => signal.removeEventListener("abort", onAbort);
  return {
    pid: child.pid,
    result: child.result.then(
      (result) => {
        settle();
        return result;
      },
      (error: unknown) => {
        settle();
        throw error;
      },
    ),
  };
}

/**
 * The QuickJS `Host`. File selection and source reads run natively in Rust
 * (`Host.selectFiles`, `Host.fs.readSources`) unless the environment sets
 * `CODEMOD_NATIVE_SELECT=0`, which walks in the interpreter instead (the
 * same result; for comparisons and benchmarks).
 */
export function createQuickjsHost(native: QuickjsNative): Host {
  const timers = globalThis as unknown as Timers;
  const nativeSelect = native.env().CODEMOD_NATIVE_SELECT !== "0";
  return {
    name: "quickjs",
    fs: {
      readText: (path, options) => native.readText(path, options?.fatal === true),
      writeText: (path, content, options) => native.writeText(path, content, options?.mode),
      mkdir: (path, options) => native.mkdir(path, options?.recursive === true),
      rm: (path, options) => native.rm(path, options?.force === true, options?.recursive === true),
      lstat: (path) => native.lstat(path),
      realpath: (path) => native.realpath(path),
      readdir: (path) => native.readdir(path),
      ...(nativeSelect
        ? {
            readSources: (root: string, paths: readonly string[]) =>
              native.readSources(root, [...paths]),
          }
        : {}),
    },
    ...(nativeSelect
      ? { selectFiles: (request: HostSelectRequest) => native.selectFiles(JSON.stringify(request)) }
      : {}),
    path: createPosixPath(() => native.cwd()),
    os: {
      platform: native.platform,
      availableParallelism: () => native.availableParallelism(),
      totalmem: () => native.totalmem(),
      homedir: () => native.homedir() ?? undefined,
      tmpdir: () => native.tmpdir(),
    },
    cwd: () => native.cwd(),
    env: () => native.env(),
    spawn: (options) => spawn(native, options),
    createAsyncContext: createPropagatingAsyncContext,
    now: () => native.now(),
    clock: { ns: () => native.clockNs() },
    setTimeout: (callback, ms) => timers.setTimeout(callback, ms),
    clearTimeout: (timer) => timers.clearTimeout(timer),
    randomUUID: () => native.randomUUID(),
  };
}

/** `console` over the host's output: strings verbatim, everything else as JSON. */
export function createQuickjsConsole(
  native: QuickjsNative,
): Record<string, (...items: unknown[]) => void> {
  const format = (items: unknown[]) =>
    items
      .map((item) => {
        if (typeof item === "string") return item;
        if (item instanceof Error) return item.stack ?? `${item.name}: ${item.message}`;
        if (item === undefined || typeof item === "function" || typeof item === "symbol") {
          return String(item);
        }
        try {
          return JSON.stringify(item) ?? String(item);
        } catch {
          return String(item);
        }
      })
      .join(" ");
  const out = (...items: unknown[]) => native.print("stdout", `${format(items)}\n`);
  const err = (...items: unknown[]) => native.print("stderr", `${format(items)}\n`);
  return { log: out, info: out, debug: out, warn: err, error: err, trace: err };
}

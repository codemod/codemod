/**
 * The primitives the Rust QuickJS host (`crates/orchestration-quickjs-host`,
 * `src/native.rs` and `src/jssg.rs`) publishes as
 * `globalThis.__codemodQuickjsNative` before the workflow bundle evaluates.
 * Runs inside QuickJS: Node-free (`tests/portable.test.ts`).
 */
import type { HostDirEntry, HostFileStat, HostSpawnResult } from "../../core/host.ts";

export const NATIVE_GLOBAL = "__codemodQuickjsNative";
/** Where the entry publishes the function the host calls on SIGINT/SIGTERM/SIGHUP. */
export const ABORT_GLOBAL = "__codemodQuickjsAbort";

export interface QuickjsNative {
  // Synchronous file system; errors carry a POSIX `code`.
  readText(path: string, fatal: boolean): string;
  writeText(path: string, content: string, mode: number | undefined): void;
  mkdir(path: string, recursive: boolean): void;
  rm(path: string, force: boolean, recursive: boolean): void;
  lstat(path: string): HostFileStat;
  realpath(path: string): string;
  readdir(path: string): HostDirEntry[];
  /** `Host.selectFiles` (`crates/orchestration-walker`); the request as JSON. */
  selectFiles(request: string): string[];
  /** `Host.fs.readSources`; files are read on worker threads. */
  readSources(root: string, paths: string[]): (string | undefined)[];

  readonly platform: string;
  availableParallelism(): number;
  totalmem(): number;
  homedir(): string | undefined;
  tmpdir(): string;
  cwd(): string;
  env(): Record<string, string>;
  /** Milliseconds since the Unix epoch. */
  now(): number;
  /** Nanoseconds since the host process started (monotonic): `Host.clock.ns()`. */
  clockNs(): number;
  /**
   * Benchmark timings are on: the host has its `timings` feature and
   * `CODEMOD_TIMINGS_FILE` is set; the entry then returns its JS-side
   * phases in the result (`core/timings.ts`).
   */
  readonly timingsEnabled: boolean;
  randomUUID(): string;
  /** Console output (the host writes it to stderr). */
  print(stream: "stdout" | "stderr", text: string): void;

  /**
   * Start a child in its own process group with exactly `env`. Never throws
   * for a start failure: `result` rejects with the error and its `code`.
   */
  spawn(
    file: string,
    args: string[],
    cwd: string,
    env: Record<string, string>,
    options: {
      stdin?: string;
      stderr?: "pipe" | "merge" | "ignore";
      onOutput?: (stream: "stdout" | "stderr", chunk: string) => void;
    },
  ): { id: number; pid?: number; result: Promise<HostSpawnResult> };
  /** SIGKILL the child's process group (no-op once it exited). */
  kill(id: number): void;
  /**
   * Settles after the host's event loop has run once (signals, child output,
   * timers). Promise jobs alone never let it run: a long stretch of
   * synchronous work plus resolved promises (a JSSG batch) must await this
   * now and then, or a SIGINT is only seen once the run is over.
   */
  yieldNow(): Promise<void>;

  /**
   * One HTTP request, only to the TypeSafe API root (`TYPESAFE_BASE_URL`,
   * default `https://api.typesafe.ai`); anything else throws `EACCES`.
   * Redirects are not followed. The body is buffered. A transport failure
   * rejects with `code: "ECONNECTION"`.
   */
  httpRequest(request: {
    url: string;
    method: string;
    headers: Record<string, string>;
    body?: string;
    timeoutMs: number;
  }): Promise<{ status: number; headers: [string, string][]; body: string }>;

  /** Make `source` importable as `import(specifier)` (evaluated once by the engine). */
  registerModule(specifier: string, source: string): void;

  /**
   * Plan a JSSG batch exactly as the execution bridge does (hash check,
   * canonical root, selector, every path contained). Throws on a bad batch.
   */
  jssgBegin(
    config: string,
    paths: string[],
  ): { id: number; module: string; indexesWorkspace: boolean };
  /** Add files `start ..< start + contents.length` to the batch's workspace index. */
  jssgIndex(batch: number, start: number, contents: string[]): void;
  /** Roots of files `start ..< start + contents.length`; `null` where the static selector skips. */
  jssgParse(batch: number, start: number, contents: string[]): unknown[];
  /** Start one transform call (staging cleared, params set); the transform's `options`. */
  jssgCall(batch: number): unknown;
  /** End the call; keeps what it staged for `jssgFinish(batch, index, ...)`. */
  jssgCallEnd(batch: number, index: number): void;
  /** The awaited transform result as `FileOutcome` JSON (`undefined`: no edits, no output). Throws `JSSG failed for '<path>': ...`. */
  jssgFinish(batch: number, index: number, root: unknown, result: unknown): string | undefined;
  jssgEnd(batch: number): void;
}

/** The host's primitives, or an error naming what is missing. */
export function quickjsNative(): QuickjsNative {
  const native = (globalThis as Record<string, unknown>)[NATIVE_GLOBAL];
  if (native === undefined) {
    throw new Error(
      `globalThis.${NATIVE_GLOBAL} is missing: this bundle runs only in the orchestration QuickJS host`,
    );
  }
  return native as QuickjsNative;
}

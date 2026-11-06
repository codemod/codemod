/**
 * The host seam: everything the portable control plane (authoring, runtime,
 * history, replay, scheduling) and the portable JSSG + shell execution path
 * need from the platform they run on. Nothing outside the Node host
 * implementation (`src/host/node-host.ts`) and the Node-only bridge/agent/
 * assessment/build modules may import `node:*` or touch `process`;
 * `tests/portable.test.ts` enforces it with a static import scan.
 *
 * Two implementations are planned:
 *
 * - Node (`createNodeHost()` in `src/host/node-host.ts`): wraps `node:fs`,
 *   `node:path`, `node:child_process`, `node:os`, and `AsyncLocalStorage`.
 *   Installed automatically by the Node entry points (`src/index.ts`,
 *   `src/host/cli.ts`, `src/host/harness.ts`).
 * - QuickJS (a Rust binary hosting one QuickJS context): backed by Rust host
 *   modules. It installs itself with `setHost()` or by defining
 *   `globalThis[HOST_GLOBAL]` before the workflow bundle is evaluated. See
 *   `RUNTIMES.md` for exactly what it must provide.
 *
 * Every filesystem call is synchronous on purpose: selection, containment,
 * staging, and the commit are synchronous today, the single JS thread is the
 * only writer, and a synchronous Rust host function is the simplest thing a
 * QuickJS host can expose. Process spawning is the only asynchronous surface.
 *
 * Errors: host functions throw `Error`s carrying a POSIX-style `code` string
 * (`ENOENT`, `ENOTDIR`, `EEXIST`, `EACCES`, ...). Callers rely on `ENOENT` and
 * on `EILSEQ` (invalid UTF-8 under `readText(path, { fatal: true })`).
 */

/** `lstat`: what a path is, without following a final symlink. */
export interface HostFileStat {
  readonly isFile: boolean;
  readonly isDirectory: boolean;
  readonly isSymbolicLink: boolean;
}

/**
 * One directory entry, typed without following symlinks: a symlink (to
 * anything) and every special file have `isFile` and `isDirectory` false.
 */
export interface HostDirEntry {
  readonly name: string;
  readonly isFile: boolean;
  readonly isDirectory: boolean;
}

export interface HostFs {
  /**
   * The file's content decoded as UTF-8 (follows symlinks). Without `fatal`,
   * invalid sequences become U+FFFD; with it, invalid UTF-8 throws an error
   * with `code: "EILSEQ"`. A missing file throws `code: "ENOENT"`.
   */
  readText(path: string, options?: { fatal?: boolean }): string;
  /** Create or truncate, UTF-8 encoded. `mode` applies when the file is created. */
  writeText(path: string, content: string, options?: { mode?: number }): void;
  mkdir(path: string, options?: { recursive?: boolean }): void;
  /** `force`: a missing path is not an error. `recursive`: remove directories. */
  rm(path: string, options?: { force?: boolean; recursive?: boolean }): void;
  lstat(path: string): HostFileStat;
  /** Canonical absolute path with every symlink resolved (`realpath(3)`). */
  realpath(path: string): string;
  /** Entries in unspecified order, without `.` and `..`. */
  readdir(path: string): HostDirEntry[];
  /**
   * Optional batch form of `readText(join(root, path), { fatal: true })` for
   * every path: the content, or `undefined` where that read throws `ENOENT`
   * or `EILSEQ`. Any other failure throws `readText`'s error for the first
   * such path. `execution/jssg.ts` uses it, when present, to read a selection.
   */
  readSources?(root: string, paths: readonly string[]): (string | undefined)[];
}

/**
 * The inputs of one `walk()` of `execution/files.ts` after the TypeScript
 * side has derived them (see `Host.selectFiles`).
 */
export interface HostSelectRequest {
  /** Directory to walk: absolute and normalized. */
  root: string;
  /**
   * Include/exclude sets in precedence order. `patterns` are lines for the
   * `ignore` package (excludes as written, includes as `!glob`, braces
   * expanded); `root` is absolute, normalized, and `root` of the request or
   * one of its ancestors; `whitelists` is true when the set has an include.
   */
  overrides: { root: string; patterns: string[]; whitelists: boolean }[];
  /** The global git excludes file, already discovered; `null` for none. */
  globalExcludes: string | null;
}

/**
 * Path operations with the host platform's conventions. Node: `node:path`.
 * Hosts without one may use `createPosixPath()` (`core/posix.ts`), a faithful
 * port of `node:path.posix`.
 */
export interface HostPath {
  readonly sep: "/" | "\\";
  resolve(...paths: string[]): string;
  join(...paths: string[]): string;
  normalize(path: string): string;
  dirname(path: string): string;
  basename(path: string): string;
  relative(from: string, to: string): string;
  isAbsolute(path: string): boolean;
}

/** Facts about the machine; they size the scheduler and locate git excludes. */
export interface HostOs {
  /** `node:os` names: `darwin`, `linux`, `win32`, ... */
  readonly platform: string;
  /** Logical CPUs this process may use (`os.availableParallelism()`). */
  availableParallelism(): number;
  /** Total memory in bytes. */
  totalmem(): number;
  /** The user's home directory, or `undefined` when unknown. */
  homedir(): string | undefined;
  /** A writable directory for temporary files. */
  tmpdir(): string;
}

export interface HostSpawnOptions {
  /** Executable: an absolute path, or a name the host resolves on `env.PATH`. */
  file: string;
  args: readonly string[];
  cwd: string;
  /** The complete child environment. Nothing is inherited implicitly. */
  env: Readonly<Record<string, string>>;
  /** Written to stdin, then closed. Without it the child's stdin is empty/ignored. */
  stdin?: string;
  /**
   * `merge` sends stderr into the stdout stream (`stderr` is then empty), as
   * `butterflow_runners::DirectRunner` does with one pipe on Unix. Hosts that
   * cannot share one pipe interleave in arrival order. Default `pipe`.
   */
  stderr?: "pipe" | "merge" | "ignore";
  /**
   * Aborting kills the child's whole process tree (process group plus
   * best-effort descendants that left it). The result still settles.
   */
  signal?: AbortSignal;
  /** Streaming: decoded UTF-8 chunks in arrival order. */
  onOutput?: (stream: "stdout" | "stderr", chunk: string) => void;
}

export interface HostSpawnResult {
  /** Exit code, or `null` when the child was terminated by a signal. */
  code: number | null;
  /** Terminating signal name (`SIGKILL`), or `null`. */
  signal: string | null;
  /** Decoded UTF-8 output (lossy). */
  stdout: string;
  stderr: string;
}

export interface HostChild {
  /** OS process id when the host knows it. */
  readonly pid: number | undefined;
  /**
   * Settles when the child has exited and its output is drained. Rejects only
   * when the child could not be started (e.g. `code: "ENOENT"`).
   */
  readonly result: Promise<HostSpawnResult>;
}

/**
 * An async-context variable, the shape of TC39 `AsyncContext.Variable`:
 * `run(value, fn)` binds `value` for `fn` and for every promise continuation
 * created while it runs (transitively); `get()` returns the binding of the
 * current execution, or `undefined` outside any `run`.
 *
 * Node: `AsyncLocalStorage`. QuickJS: see `RUNTIMES.md` (promise hooks;
 * `createFallbackAsyncContext()` in `core/async-context.ts` as a stopgap).
 */
export interface AsyncContextVariable<T> {
  get(): T | undefined;
  run<R>(value: T, fn: () => R): R;
}

/** Opaque timer handle; only ever passed back to `clearTimeout`. */
export type HostTimer = unknown;

export interface Host {
  /** Implementation name for diagnostics: `node`, `quickjs`, ... */
  readonly name: string;
  readonly fs: HostFs;
  readonly path: HostPath;
  readonly os: HostOs;
  /** The process working directory (absolute). */
  cwd(): string;
  /** The process environment. Callers copy before modifying. */
  env(): Readonly<Record<string, string | undefined>>;
  /** Start a child process. Never throws; start failures reject `result`. */
  spawn(options: HostSpawnOptions): HostChild;
  /** A fresh async-context variable (see `AsyncContextVariable`). */
  createAsyncContext<T>(): AsyncContextVariable<T>;
  /** Milliseconds since the Unix epoch. */
  now(): number;
  setTimeout(callback: () => void, ms: number): HostTimer;
  clearTimeout(timer: HostTimer): void;
  /** RFC 4122 v4 UUID; used for unique temporary names. */
  randomUUID(): string;
  /**
   * Optional native file selection: `walk()` of `execution/files.ts` with
   * exactly its semantics and errors (the `ignore` package's matching, every
   * ancestor's ignore files, the override sets, lossy entry names, symlinks
   * skipped), returning target-relative `/` paths sorted by `comparePaths`.
   * `files.ts` uses it when present and falls back to walking through `fs`.
   * The QuickJS host implements it in Rust (`crates/orchestration-walker`);
   * `tests/files-native.test.ts` checks it against the TypeScript walker.
   */
  readonly selectFiles?: (request: HostSelectRequest) => string[];
}

/**
 * Where a host that cannot run an installer first (a QuickJS context that
 * evaluates the workflow bundle) may publish itself before evaluation.
 */
export const HOST_GLOBAL = "__codemodOrchestrationHost";

let installed: Host | undefined;

/** Thrown when portable code needs the platform before any host was installed. */
export class HostNotInstalledError extends Error {
  constructor() {
    super(
      `no orchestration host is installed; import '@codemod.com/orchestration' (Node), call setHost(), or define globalThis.${HOST_GLOBAL}`,
    );
    this.name = "HostNotInstalledError";
  }
}

/** Install the host for this JS realm. Returns the previously installed one. */
export function setHost(host: Host): Host | undefined {
  const previous = installed;
  installed = host;
  return previous;
}

/** Install `host` unless one is installed already (what entry points do). */
export function installDefaultHost(host: Host): void {
  if (findHost() === undefined) installed = host;
}

/** The installed host, or `undefined`. */
export function findHost(): Host | undefined {
  return installed ?? ((globalThis as Record<string, unknown>)[HOST_GLOBAL] as Host | undefined);
}

/** The installed host. Throws `HostNotInstalledError` when there is none. */
export function getHost(): Host {
  const host = findHost();
  if (host === undefined) throw new HostNotInstalledError();
  return host;
}

/** `code` of a host (or Node) error, when it has one. */
export function errorCode(error: unknown): string | undefined {
  const code = (error as { code?: unknown } | null)?.code;
  return typeof code === "string" ? code : undefined;
}

/**
 * The Node implementation of the host seam (`core/host.ts`): the only place
 * the portable control plane and the portable JSSG + shell path meet
 * `node:*`. Importing this module installs it (see the bottom of the file).
 */
import { AsyncLocalStorage } from "node:async_hooks";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { availableParallelism, homedir, tmpdir, totalmem } from "node:os";
import path from "node:path";
import {
  installDefaultHost,
  type AsyncContextVariable,
  type Host,
  type HostChild,
  type HostSpawnOptions,
  type HostSpawnResult,
} from "../../src/core/host.ts";
import type { SchedulerHost } from "../../src/execution/scheduler.ts";
import { killProcessTree, spawnOptions, trackProcessTree } from "./process-tree.ts";

// `ignoreBOM`: keep a leading U+FEFF, as `readFileSync(file, "utf8")`, the
// QuickJS host, and the workflow engine's `read_to_string` do (`Host.fs.readText`).
const strict = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** `node:os` capacity facts, injectable into `AdmissionScheduler` / `defaultCapacity`. */
export const nodeHost: SchedulerHost = { availableParallelism, totalmem };

function spawnChild(options: HostSpawnOptions): HostChild {
  let pid: number | undefined;
  const result = new Promise<HostSpawnResult>((resolve, reject) => {
    const merge = options.stderr === "merge";
    const child = spawn(options.file, [...options.args], {
      ...spawnOptions(),
      cwd: options.cwd,
      env: { ...options.env },
      stdio: [
        options.stdin === undefined ? "ignore" : "pipe",
        "pipe",
        options.stderr === "ignore" ? "ignore" : "pipe",
      ],
    });
    pid = child.pid;
    if (options.stdin !== undefined && child.stdin !== null) {
      child.stdin.on("error", () => {});
      child.stdin.end(options.stdin);
    }
    const untrack = trackProcessTree(child.pid);
    const out: string[] = [];
    const err: string[] = [];
    const collect = (stream: "stdout" | "stderr") => (chunk: string) => {
      (stream === "stdout" || merge ? out : err).push(chunk);
      options.onOutput?.(merge ? "stdout" : stream, chunk);
    };
    child.stdout?.setEncoding("utf8").on("data", collect("stdout"));
    child.stderr?.setEncoding("utf8").on("data", collect("stderr"));
    const onAbort = () => killProcessTree(child.pid);
    if (options.signal?.aborted) onAbort();
    else options.signal?.addEventListener("abort", onAbort, { once: true });
    const settle = () => {
      options.signal?.removeEventListener("abort", onAbort);
      untrack();
    };
    child.once("error", (error) => {
      settle();
      reject(error);
    });
    child.once("close", (code, signal) => {
      settle();
      resolve({ code, signal, stdout: out.join(""), stderr: err.join("") });
    });
  });
  return {
    get pid() {
      return pid;
    },
    result,
  };
}

export function createNodeHost(): Host {
  return {
    fs: {
      readText(file, options) {
        if (!options?.fatal) return readFileSync(file, "utf8");
        const bytes = readFileSync(file);
        try {
          return strict.decode(bytes);
        } catch {
          throw Object.assign(new Error(`'${file}' is not valid UTF-8`), { code: "EILSEQ" });
        }
      },
      writeText(file, content, options) {
        writeFileSync(
          file,
          content,
          options?.mode === undefined ? undefined : { mode: options.mode },
        );
      },
      mkdir(dir, options) {
        mkdirSync(dir, options);
      },
      rm(target, options) {
        rmSync(target, options);
      },
      lstat(target) {
        const stat = lstatSync(target);
        return {
          isFile: stat.isFile(),
          isDirectory: stat.isDirectory(),
          isSymbolicLink: stat.isSymbolicLink(),
        };
      },
      realpath: (target) => realpathSync.native(target),
      readdir: (dir) =>
        readdirSync(dir, { withFileTypes: true }).map((entry) => ({
          name: entry.name,
          isFile: entry.isFile(),
          isDirectory: entry.isDirectory(),
        })),
    },
    path: {
      sep: path.sep as "/" | "\\",
      resolve: path.resolve,
      join: path.join,
      normalize: path.normalize,
      dirname: path.dirname,
      basename: (p) => path.basename(p),
      relative: path.relative,
      isAbsolute: path.isAbsolute,
    },
    os: {
      platform: process.platform,
      availableParallelism,
      totalmem,
      homedir: () => homedir() || undefined,
      tmpdir,
    },
    cwd: () => process.cwd(),
    env: () => process.env,
    spawn: spawnChild,
    createAsyncContext<T>(): AsyncContextVariable<T> {
      const storage = new AsyncLocalStorage<T>();
      return { get: () => storage.getStore(), run: (value, fn) => storage.run(value, fn) };
    },
    now: () => Date.now(),
    setTimeout: (callback, ms) => setTimeout(callback, ms),
    clearTimeout: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
    randomUUID: () => randomUUID(),
  };
}

/** The process-wide Node host. */
export const NODE_HOST: Host = createNodeHost();

// Importing this module (every Node entry point does) installs the Node host
// unless the embedder installed another one first.
installDefaultHost(NODE_HOST);

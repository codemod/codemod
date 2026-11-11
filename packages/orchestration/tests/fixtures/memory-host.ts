/**
 * An in-memory `Host` with no Node behind it: posix paths from
 * `createPosixPath`, a Map-backed filesystem, `AsyncLocalStorage`, and
 * no process spawning. Proves the portable JSSG path reaches the platform only
 * through the seam: a test can run selection, staging, and the commit against
 * it and never touch the real disk.
 */
import { AsyncLocalStorage } from "node:async_hooks";
import { createPosixPath, type Host, type HostDirEntry } from "../../src/portable.ts";

type Node = { type: "file"; content: string } | { type: "dir" } | { type: "link"; to: string };

const fsError = (code: string, message: string) => Object.assign(new Error(message), { code });

export interface MemoryHost extends Host {
  /** Files by absolute path. */
  files(): Record<string, string>;
  write(path: string, content: string): void;
  symlink(path: string, to: string): void;
  /** Every filesystem call, in order (`op path`). */
  readonly calls: string[];
}

export function createMemoryHost(cwd = "/repo"): MemoryHost {
  const path = createPosixPath(() => cwd);
  const nodes = new Map<string, Node>([["/", { type: "dir" }]]);
  const calls: string[] = [];
  const ensureDir = (dir: string) => {
    for (let at = dir; !nodes.has(at); at = path.dirname(at)) nodes.set(at, { type: "dir" });
  };
  /** Follow symlinks through every component. */
  const real = (target: string, depth = 0): string => {
    if (depth > 32) throw fsError("ELOOP", target);
    const parts = path.resolve(target).split("/").filter(Boolean);
    let at = "/";
    for (const part of parts) {
      at = path.join(at, part);
      const node = nodes.get(at);
      if (node?.type === "link") at = real(path.resolve(path.dirname(at), node.to), depth + 1);
    }
    return at;
  };
  const get = (target: string): Node => {
    const node = nodes.get(real(target));
    if (node === undefined) throw fsError("ENOENT", `no such file: ${target}`);
    return node;
  };
  const host: MemoryHost = {
    calls,
    fs: {
      readText(target, options) {
        calls.push(`readText ${target}`);
        const node = get(target);
        if (node.type !== "file") throw fsError("EISDIR", target);
        if (options?.fatal && node.content.includes("�")) throw fsError("EILSEQ", target);
        return node.content;
      },
      writeText(target, content) {
        calls.push(`writeText ${target}`);
        const at = real(path.dirname(target));
        if (nodes.get(at)?.type !== "dir") throw fsError("ENOENT", target);
        nodes.set(path.join(at, path.basename(target)), { type: "file", content });
      },
      mkdir(target, options) {
        calls.push(`mkdir ${target}`);
        if (options?.recursive) ensureDir(real(target));
        else nodes.set(real(target), { type: "dir" });
      },
      rm(target, options) {
        calls.push(`rm ${target}`);
        const at = path.resolve(target);
        if (!nodes.has(at)) {
          if (options?.force) return;
          throw fsError("ENOENT", target);
        }
        for (const key of [...nodes.keys()]) {
          if (key === at || key.startsWith(`${at}/`)) nodes.delete(key);
        }
      },
      lstat(target) {
        const node = nodes.get(path.join(real(path.dirname(target)), path.basename(target)));
        if (node === undefined) throw fsError("ENOENT", target);
        return {
          isFile: node.type === "file",
          isDirectory: node.type === "dir",
          isSymbolicLink: node.type === "link",
        };
      },
      realpath(target) {
        const at = real(target);
        if (!nodes.has(at)) throw fsError("ENOENT", target);
        return at;
      },
      readdir(target) {
        calls.push(`readdir ${target}`);
        const dir = real(target);
        if (nodes.get(dir)?.type !== "dir") throw fsError("ENOTDIR", target);
        const entries: HostDirEntry[] = [];
        for (const [key, node] of nodes) {
          if (key !== dir && path.dirname(key) === dir) {
            entries.push({
              name: path.basename(key),
              isFile: node.type === "file",
              isDirectory: node.type === "dir",
            });
          }
        }
        return entries;
      },
    },
    path,
    os: {
      platform: "linux",
      availableParallelism: () => 4,
      totalmem: () => 8 * 1024 ** 3,
      homedir: () => undefined,
      tmpdir: () => "/tmp",
    },
    cwd: () => cwd,
    env: () => ({}),
    spawn: () => ({
      pid: undefined,
      result: Promise.reject(fsError("ENOSYS", "the memory host cannot spawn")),
    }),
    createAsyncContext<T>() {
      const storage = new AsyncLocalStorage<T>();
      return {
        get: () => storage.getStore(),
        run: <R>(value: T, fn: () => R) => storage.run(value, fn),
      };
    },
    now: () => 0,
    setTimeout: (callback, ms) => setTimeout(callback, ms),
    clearTimeout: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
    randomUUID: () => "00000000-0000-4000-8000-000000000000",
    files() {
      return Object.fromEntries(
        [...nodes].flatMap(([key, node]) => (node.type === "file" ? [[key, node.content]] : [])),
      );
    },
    write(target, content) {
      ensureDir(path.dirname(target));
      nodes.set(target, { type: "file", content });
    },
    symlink(target, to) {
      ensureDir(path.dirname(target));
      nodes.set(target, { type: "link", to });
    },
  };
  return host;
}

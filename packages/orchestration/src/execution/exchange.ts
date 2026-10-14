/**
 * Where bridge request/response files live, and how the response is read.
 *
 * The exchange must not be writable by what the bridge runs. A Codex
 * `workspace-write` sandbox may write the target, its `TMPDIR`, and (unless
 * excluded) `/tmp`; the bridge gives Codex a private `TMPDIR` and excludes
 * `/tmp`, and the exchange lives in a host-owned per-user directory outside
 * all of them:
 *
 * 1. `CODEMOD_BRIDGE_EXCHANGE_DIR`, when set (an absolute path; if it is
 *    unusable the bridge is not started rather than falling back);
 * 2. the platform's per-user runtime or cache directory: macOS
 *    `~/Library/Caches/codemod/bridge`; other POSIX `$XDG_RUNTIME_DIR/codemod/bridge`,
 *    else `${XDG_CACHE_HOME:-~/.cache}/codemod/bridge`; Windows
 *    `%LOCALAPPDATA%\codemod\bridge`;
 * 3. `<os tmpdir>/codemod-bridge-<uid>` when none of those can be created
 *    (read-only home, restricted sandboxes).
 *
 * A candidate is used only if it is an absolute, real directory (not a
 * symlink), owned by the current user with no group or other permissions
 * (POSIX; tightened to 0700 when owned but looser), not inside the bridge's
 * working directory, which is the target, and actually writable (a probe
 * directory is created and removed); otherwise the next candidate is tried. Each exchange is a fresh
 * `mkdtemp` directory (0700) with the request created exclusively (0600).
 * The bridge creates the response exclusively too; the host opens it with
 * `O_NOFOLLOW` and accepts only a regular file of bounded size.
 *
 * Limits: an opted-in Claude Code `Bash` tool or a builtin `bash` tool is not
 * sandboxed and runs as the same user, so it can reach any of these paths.
 * Windows has no ownership or mode check here.
 */
import {
  chmodSync,
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { isAbsolute, join, relative } from "node:path";

export const EXCHANGE_DIR_ENV = "CODEMOD_BRIDGE_EXCHANGE_DIR";

/** Responses larger than this are rejected rather than read. */
export const RESPONSE_SIZE_LIMIT = 512 * 1024 * 1024;

export interface ExchangeHost {
  env: Record<string, string | undefined>;
  platform: NodeJS.Platform;
  home: string;
  tmp: string;
  /** `process.getuid()`; undefined on Windows. */
  uid: number | undefined;
}

export const nodeExchangeHost = (): ExchangeHost => ({
  env: process.env,
  platform: process.platform,
  home: homedir(),
  tmp: tmpdir(),
  uid: typeof process.getuid === "function" ? process.getuid() : undefined,
});

/** Candidate exchange roots, most preferred first. */
export function exchangeRootCandidates(host: ExchangeHost): string[] {
  const { env, platform, home, tmp, uid } = host;
  const candidates: string[] = [];
  const add = (path: string | undefined) => {
    if (path !== undefined && path.trim() !== "" && isAbsolute(path)) candidates.push(path);
  };
  add(env[EXCHANGE_DIR_ENV]);
  if (platform === "darwin") {
    add(join(home, "Library", "Caches", "codemod", "bridge"));
  } else if (platform === "win32") {
    if (env.LOCALAPPDATA) add(join(env.LOCALAPPDATA, "codemod", "bridge"));
  } else {
    if (env.XDG_RUNTIME_DIR) add(join(env.XDG_RUNTIME_DIR, "codemod", "bridge"));
    add(join(env.XDG_CACHE_HOME || join(home, ".cache"), "codemod", "bridge"));
  }
  add(join(tmp, `codemod-bridge-${uid ?? "user"}`));
  return candidates;
}

/** Why `root` cannot hold exchanges for a bridge running in `cwd`, or undefined. */
export function exchangeRootProblem(
  root: string,
  cwd: string,
  uid: number | undefined,
  platform: NodeJS.Platform,
): string | undefined {
  let stat;
  try {
    stat = lstatSync(root);
  } catch (error) {
    return `cannot stat: ${(error as Error).message}`;
  }
  if (stat.isSymbolicLink()) return "is a symbolic link";
  if (!stat.isDirectory()) return "is not a directory";
  if (platform !== "win32" && uid !== undefined) {
    if (stat.uid !== uid) return "is not owned by the current user";
    if ((stat.mode & 0o077) !== 0) return "is accessible by other users";
  }
  try {
    const inside = relative(realpathSync(cwd), realpathSync(root));
    if (inside === "" || (!inside.startsWith("..") && !isAbsolute(inside))) {
      return "is inside the bridge working directory";
    }
  } catch (error) {
    return `cannot resolve: ${(error as Error).message}`;
  }
  return undefined;
}

/** The first usable exchange root, created and tightened as needed. */
export function resolveExchangeRoot(
  cwd: string,
  host: ExchangeHost = nodeExchangeHost(),
): { root: string } | { problem: string } {
  const problems: string[] = [];
  for (const candidate of exchangeRootCandidates(host)) {
    try {
      mkdirSync(candidate, { recursive: true, mode: 0o700 });
      const stat = lstatSync(candidate);
      if (
        host.platform !== "win32" &&
        stat.isDirectory() &&
        !stat.isSymbolicLink() &&
        stat.uid === host.uid &&
        (stat.mode & 0o077) !== 0
      ) {
        // Our own directory with loose permissions (for example a pre-existing cache dir).
        chmodSync(candidate, 0o700);
      }
    } catch (error) {
      problems.push(`${candidate}: ${(error as Error).message}`);
      if (candidate === host.env[EXCHANGE_DIR_ENV]) break;
      continue;
    }
    const problem =
      exchangeRootProblem(candidate, cwd, host.uid, host.platform) ?? writeProblem(candidate);
    if (problem === undefined) return { root: candidate };
    problems.push(`${candidate} ${problem}`);
    // An explicit choice is not silently replaced by a default.
    if (candidate === host.env[EXCHANGE_DIR_ENV]) break;
  }
  return { problem: `no usable bridge exchange directory (${problems.join("; ")})` };
}

/**
 * Ownership and mode do not prove the host can write there (read-only mounts,
 * OS sandboxes): create and remove one private directory to be sure.
 */
function writeProblem(root: string): string | undefined {
  try {
    rmSync(mkdtempSync(join(root, "probe-")), { recursive: true, force: true });
    return undefined;
  } catch (error) {
    return `is not writable: ${(error as Error).message}`;
  }
}

export interface Exchange {
  dir: string;
  requestPath: string;
  responsePath: string;
  cleanup(): void;
}

/** A fresh private exchange directory with the request written exclusively. */
export function createExchange(root: string, request: string): Exchange {
  const dir = mkdtempSync(join(root, "x-"));
  const requestPath = join(dir, "request.json");
  const responsePath = join(dir, "response.json");
  try {
    writeFileSync(requestPath, request, { flag: "wx", mode: 0o600 });
  } catch (error) {
    rmSync(dir, { recursive: true, force: true });
    throw error;
  }
  return {
    dir,
    requestPath,
    responsePath,
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}

export type ResponseRead =
  | { kind: "text"; text: string }
  | { kind: "missing" }
  | { kind: "rejected"; reason: string };

/**
 * Read the bridge's response without following a symlink: only a regular
 * file of at most `RESPONSE_SIZE_LIMIT` bytes is accepted.
 */
export function readResponse(path: string, limit = RESPONSE_SIZE_LIMIT): ResponseRead {
  let link;
  try {
    link = lstatSync(path);
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "ENOENT"
      ? { kind: "missing" }
      : { kind: "rejected", reason: (error as Error).message };
  }
  if (link.isSymbolicLink()) return { kind: "rejected", reason: "response is a symbolic link" };
  if (!link.isFile()) return { kind: "rejected", reason: "response is not a regular file" };
  let fd: number;
  try {
    fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  } catch (error) {
    return {
      kind: "rejected",
      reason: `response could not be opened: ${(error as Error).message}`,
    };
  }
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile()) return { kind: "rejected", reason: "response is not a regular file" };
    if (stat.size > limit) {
      return { kind: "rejected", reason: `response is larger than ${limit} bytes` };
    }
    return { kind: "text", text: readFileSync(fd, "utf8") };
  } finally {
    closeSync(fd);
  }
}

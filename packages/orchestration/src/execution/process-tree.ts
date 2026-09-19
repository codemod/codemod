/**
 * Best-effort termination of a bridge process and everything it started.
 *
 * Killing only the bridge would orphan its children: the shell a `shell`
 * command runs, or the session the agent's bash tool keeps in a process group
 * of its own. So, on POSIX, the bridge is spawned as a process-group leader
 * (`spawnOptions`), and `killProcessTree`:
 *
 * 1. stops the bridge's whole process group (SIGSTOP), so nothing in it can
 *    fork, exit, or reap a child from here on;
 * 2. takes one `ps` snapshot (1 s timeout) and stops the descendants found
 *    by parent id outside that group, parents before children;
 * 3. SIGKILLs those descendants, children before parents, then the group.
 *
 * PID reuse: a descendant is signaled by number only after its parent was
 * stopped, and a stopped parent cannot reap it, so if it exits it stays a
 * zombie and keeps its PID until the kill. The remaining window is between the
 * snapshot and stopping a parent outside the bridge's group; PIDs would have to
 * wrap around within those milliseconds.
 *
 * On Windows `taskkill /T /F` walks the tree by parent id.
 *
 * Limits, by design and documented: a descendant that detached from its
 * parent (double fork, daemon) cannot be found; without `ps` (restricted
 * sandboxes) only the bridge's process group is reached; nothing runs when
 * the host is killed with SIGKILL or crashes. The host's `exit` event kills
 * every tree still tracked; hosts must turn SIGINT/SIGTERM/SIGHUP into an
 * abort (or a normal exit) for that to happen.
 */
import { spawnSync, type SpawnOptions } from "node:child_process";

const SNAPSHOT_TIMEOUT_MS = 1_000;

const live = new Set<number>();
let exitHookInstalled = false;

/** Spawn options that make the child killable as a tree on this platform. */
export function spawnOptions(platform: NodeJS.Platform = process.platform): SpawnOptions {
  return platform === "win32" ? { windowsHide: true } : { detached: true };
}

/** Track a spawned tree so the host's `exit` kills it if it is still running. */
export function trackProcessTree(pid: number | undefined): () => void {
  if (pid === undefined) return () => {};
  live.add(pid);
  if (!exitHookInstalled) {
    exitHookInstalled = true;
    process.once("exit", () => {
      for (const tracked of live) killProcessTree(tracked);
    });
  }
  return () => live.delete(pid);
}

export function killProcessTree(
  pid: number | undefined,
  platform: NodeJS.Platform = process.platform,
  list: () => ProcessTable | undefined = processTable,
): void {
  if (pid === undefined) return;
  if (platform === "win32") {
    const result = spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], {
      windowsHide: true,
      timeout: 5_000,
    });
    if (result.status !== 0) signal(pid, "SIGKILL");
    return;
  }
  signal(-pid, "SIGSTOP");
  signal(pid, "SIGSTOP");
  const table = list();
  const outside =
    table === undefined
      ? []
      : descendants(pid, table.parents).filter((child) => table.groups.get(child) !== pid);
  for (const child of outside) signal(child, "SIGSTOP");
  for (const child of outside.reverse()) signal(child, "SIGKILL");
  signal(-pid, "SIGKILL");
  signal(pid, "SIGKILL");
}

function signal(pid: number, name: NodeJS.Signals): void {
  try {
    process.kill(pid, name);
  } catch {
    // Already gone, or not ours to signal.
  }
}

export interface ProcessTable {
  /** pid -> parent pid */
  parents: ReadonlyMap<number, number>;
  /** pid -> process group id */
  groups: ReadonlyMap<number, number>;
}

/** One listing of every visible process, or undefined when listing is not possible. */
function processTable(): ProcessTable | undefined {
  const result = spawnSync("ps", ["-A", "-o", "pid=,ppid=,pgid="], {
    encoding: "utf8",
    timeout: SNAPSHOT_TIMEOUT_MS,
  });
  if (result.status !== 0 || typeof result.stdout !== "string") return undefined;
  const parents = new Map<number, number>();
  const groups = new Map<number, number>();
  for (const line of result.stdout.split("\n")) {
    const [child, parent, group] = line.trim().split(/\s+/u).map(Number);
    if (Number.isInteger(child) && Number.isInteger(parent) && Number.isInteger(group)) {
      parents.set(child!, parent!);
      groups.set(child!, group!);
    }
  }
  return parents.size === 0 ? undefined : { parents, groups };
}

/** Descendants of `root`, breadth first: every parent precedes its children. */
export function descendants(root: number, parents: ReadonlyMap<number, number>): number[] {
  const children = new Map<number, number[]>();
  for (const [child, parent] of parents) {
    if (child === root) continue;
    const list = children.get(parent);
    if (list === undefined) children.set(parent, [child]);
    else list.push(child);
  }
  const found: number[] = [];
  const seen = new Set<number>([root]);
  for (let queue = [root]; queue.length > 0; ) {
    const next: number[] = [];
    for (const parent of queue) {
      for (const child of children.get(parent) ?? []) {
        if (seen.has(child)) continue;
        seen.add(child);
        found.push(child);
        next.push(child);
      }
    }
    queue = next;
  }
  return found;
}

/** Whether this host can list processes (tests of cross-group cleanup need it). */
export function canListProcesses(): boolean {
  return processTable() !== undefined;
}

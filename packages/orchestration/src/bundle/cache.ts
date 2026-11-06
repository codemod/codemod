/**
 * The on-disk build cache shared by the Node side of every runtime:
 *
 * - `modules/`: workflow modules rewritten by the build step (`loadWorkflow`),
 *   with their artifacts, so a warm run loads neither TypeScript nor esbuild;
 * - `quickjs/`: prebuilt QuickJS bundles (`host/quickjs-bundle.ts`);
 * - `ts/` and `v8/`: type-stripped sources and Node's compile cache, written
 *   by `bin/register-ts.mjs` and `bin/codemod-workflow.mjs` (plain `.mjs`
 *   that runs before any TypeScript loads, so it repeats `cacheRoot` and
 *   `privateDirectory` below; keep them in sync).
 *
 * Entries are keyed by content hashes and validated against every input
 * file on use, so stale entries are ignored rather than trusted. The root is
 * `$CODEMOD_WORKFLOW_CACHE`, else `<tmpdir>/codemod-workflow-cache-<uid>`. A
 * directory that is not a real directory owned by this user, or that others
 * can write to, is never used: the cache holds code this process runs.
 */
import { createHash } from "node:crypto";
import { lstatSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** The cache root (it may not exist yet). */
export function cacheRoot(env: Record<string, string | undefined> = process.env): string {
  const configured = env.CODEMOD_WORKFLOW_CACHE;
  if (configured !== undefined && configured !== "") return configured;
  return join(tmpdir(), `codemod-workflow-cache-${process.getuid?.() ?? "user"}`);
}

/**
 * `<root>/<name>`, created (mode 0700) if missing; `undefined` when the root
 * or the directory is not private to this user (then nothing is cached).
 */
export function privateDirectory(name: string, root: string = cacheRoot()): string | undefined {
  const directory = join(root, name);
  try {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    for (const path of [root, directory]) {
      const stat = lstatSync(path);
      if (!stat.isDirectory() || (stat.mode & 0o022) !== 0) return undefined;
      if (process.getuid !== undefined && stat.uid !== process.getuid()) return undefined;
    }
    return directory;
  } catch {
    return undefined;
  }
}

export const sha256 = (...parts: (string | Uint8Array)[]): string => {
  const hash = createHash("sha256");
  for (const [index, part] of parts.entries()) {
    if (index > 0) hash.update("\0");
    hash.update(part);
  }
  return hash.digest("hex");
};

/** SHA-256 of a file's bytes, or `undefined` when it cannot be read. */
export function fileHash(path: string): string | undefined {
  try {
    return sha256(readFileSync(path));
  } catch {
    return undefined;
  }
}

/** True when every recorded input still has the recorded content hash. */
export function inputsUnchanged(inputs: Record<string, string>): boolean {
  return Object.entries(inputs).every(([input, hash]) => fileHash(input) === hash);
}

/** Write via a temporary file and a rename, so readers never see a partial entry. */
export function atomicWrite(path: string, content: string): void {
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`;
  writeFileSync(temporary, content, { mode: 0o600 });
  renameSync(temporary, path);
}

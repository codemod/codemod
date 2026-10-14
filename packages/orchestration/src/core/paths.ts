/**
 * Path rules shared by JSSG targets, JSSG definitions
 * (`semanticAnalysis.root`), the wire protocol, and the commit. The Rust
 * bridge applies the same rules (`validate_relative_path` and the containment
 * checks in `crates/execution-bridge/src/jssg.rs`), so a value accepted here
 * is accepted there and every path is checked independently on both sides.
 */
import { lstatSync, realpathSync } from "node:fs";
import { dirname, resolve, sep } from "node:path";

/** Absolute on any platform: `/x`, `\x`, `C:\x`, `c:/x`. */
export function isAbsolutePath(path: string): boolean {
  return path.startsWith("/") || path.startsWith("\\") || /^[A-Za-z]:/.test(path);
}

/** Contains a `..` segment. A `..` inside a name such as `foo..bar` is fine. */
export function escapesRoot(path: string): boolean {
  return path.split(/[\\/]/).includes("..");
}

/** Non-empty, relative, and without `..` segments. */
export function isSafeRelativePath(path: string): boolean {
  return path.trim() !== "" && !isAbsolutePath(path) && !escapesRoot(path);
}

/** A path was outside the root it must stay beneath. */
export class PathEscapeError extends Error {
  constructor(
    readonly what: string,
    readonly path: string,
    reason: string,
  ) {
    super(`${what} '${path}' ${reason}`);
    this.name = "PathEscapeError";
  }
}

/** True when `path` exists as anything, including a dangling symlink. */
export function lexists(path: string): boolean {
  try {
    lstatSync(path);
    return true;
  } catch {
    return false;
  }
}

/**
 * Resolve `relative` beneath `root` (an absolute real path) and prove it stays
 * there: the path must be a safe relative path and the real path of its
 * nearest existing ancestor (or of the path itself) must lie inside `root`.
 * This rejects `..`, absolute forms, Windows drive/UNC forms, symlinked files
 * and directories that point outside, and dangling symlinks. Returns the
 * absolute path with the existing prefix resolved.
 */
export function resolveInsideRoot(root: string, relative: string, what: string): string {
  if (!isSafeRelativePath(relative)) {
    throw new PathEscapeError(what, relative, "must be a safe relative path");
  }
  const absolute = resolve(root, relative);
  let existing = absolute;
  while (!lexists(existing)) {
    const parent = dirname(existing);
    if (parent === existing) throw new PathEscapeError(what, relative, "has no existing ancestor");
    existing = parent;
  }
  let real: string;
  try {
    real = realpathSync.native(existing);
  } catch {
    throw new PathEscapeError(what, relative, "resolves through a broken symlink");
  }
  if (real !== root && !real.startsWith(root.endsWith(sep) ? root : root + sep)) {
    throw new PathEscapeError(what, relative, "escapes the target root");
  }
  return real + absolute.slice(existing.length);
}

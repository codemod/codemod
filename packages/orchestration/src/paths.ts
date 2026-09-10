/**
 * Path rules shared by JSSG targets, JSSG definitions (`script`,
 * `semanticAnalysis.root`), and the wire protocol. The Rust bridge applies the
 * same rules in `validate_relative_path` (crates/execution-bridge), so a value
 * accepted here is accepted there.
 */

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

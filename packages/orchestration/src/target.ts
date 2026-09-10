/**
 * Author-facing validation of a JSSG invocation target. The wire type lives in
 * `protocol.ts`; this module decides what an author may write and normalizes
 * it so equivalent spellings produce identical command content for replay.
 */
import { posix } from "node:path";
import { TargetValidationError } from "./errors.ts";
import type { Target } from "./protocol.ts";

const FIELDS: readonly string[] = ["root", "include", "exclude"];

/**
 * Rules:
 * - `root` is a relative directory inside the repository. It is normalized to
 *   posix form without `./` or a trailing slash; `.` means the whole repository.
 * - `include` and `exclude` are non-empty lists of non-empty relative globs.
 * - At least one field must be present. An empty target would look like a
 *   narrowing while selecting everything, so it is rejected instead.
 */
export function normalizeTarget(value: unknown, where: string): Target {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TargetValidationError(
      where,
      "target must be an object with root, include, or exclude",
    );
  }
  const record = value as Record<string, unknown>;
  for (const key of Object.keys(record)) {
    if (!FIELDS.includes(key))
      throw new TargetValidationError(where, `unknown target field '${key}'`);
  }
  const target: Target = {};
  if (record.root !== undefined) target.root = normalizeRoot(record.root, where);
  if (record.include !== undefined) target.include = patterns(record.include, "include", where);
  if (record.exclude !== undefined) target.exclude = patterns(record.exclude, "exclude", where);
  if (Object.keys(target).length === 0) {
    throw new TargetValidationError(
      where,
      "target must set at least one of root, include, or exclude",
    );
  }
  return target;
}

function normalizeRoot(value: unknown, where: string): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new TargetValidationError(where, "root must be a non-empty relative path");
  }
  if (isAbsolute(value)) {
    throw new TargetValidationError(where, `root '${value}' must be relative to the repository`);
  }
  const normalized = posix.normalize(value.replaceAll("\\", "/")).replace(/\/+$/, "");
  if (normalized === "" || escapes(normalized)) {
    throw new TargetValidationError(where, `root '${value}' escapes the repository`);
  }
  return normalized;
}

function patterns(value: unknown, field: "include" | "exclude", where: string): string[] {
  if (!Array.isArray(value) || value.length === 0) {
    throw new TargetValidationError(where, `${field} must be a non-empty list of glob patterns`);
  }
  return value.map((pattern) => {
    if (typeof pattern !== "string" || pattern.trim() === "") {
      throw new TargetValidationError(where, `${field} patterns must be non-empty strings`);
    }
    if (isAbsolute(pattern) || escapes(pattern)) {
      throw new TargetValidationError(
        where,
        `${field} pattern '${pattern}' must stay relative to the target root`,
      );
    }
    return pattern;
  });
}

function isAbsolute(path: string): boolean {
  return path.startsWith("/") || path.startsWith("\\") || /^[A-Za-z]:/.test(path);
}

function escapes(path: string): boolean {
  return path.split(/[\\/]/).includes("..");
}

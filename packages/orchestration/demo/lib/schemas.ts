/**
 * Schemas shared by the demo workflows. `guard()` is the package's tiny
 * predicate schema; any Standard Schema library (zod, valibot, ...) works in
 * its place. A step's `input`/`output` schema types its data for TypeScript
 * and validates it at run time, so the flow between stages is explicit.
 *
 * JSSG outputs are aggregates: the transform returns one item per file and
 * the step's output is the array of them, in file order.
 */
import { guard } from "@codemod.com/orchestration";

/** One selected file's call counts, from the `api-usage` analyzer. */
export interface Usage {
  file: string;
  oldApi: number;
  newApi: number;
}

/** A file calling the API under analysis and how many call sites it has. */
export interface Finding {
  file: string;
  calls: number;
}

/** A file a migration rewrote and how many call sites changed. */
export interface Migration {
  file: string;
  replaced: number;
}

/** What `inspect` reports: files still calling the deprecated API. */
export interface Inventory {
  pending: number;
}

/** What `verify-no-legacy` reports: `oldApi(` call sites left in the sources. */
export interface Remaining {
  remaining: number;
}

/** What `verify-migrations` reports: the migration's own count against the sources. */
export interface Verification extends Remaining {
  migrated: number;
}

/** Root input of `07-input.ts`: the name every deprecated call is rewritten to. */
export interface Config {
  replacement: string;
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const isFileRecord = (value: unknown, counts: string[]): value is Record<string, unknown> =>
  isRecord(value) &&
  typeof value.file === "string" &&
  counts.every((key) => typeof value[key] === "number");

const arrayOf = <T>(name: string, item: (value: unknown) => value is T) =>
  guard(name, (value: unknown): value is T[] => Array.isArray(value) && value.every(item));

export const Usages = arrayOf("Usage[]", (v): v is Usage => isFileRecord(v, ["oldApi", "newApi"]));
export const Findings = arrayOf("Finding[]", (v): v is Finding => isFileRecord(v, ["calls"]));
export const Migrations = arrayOf("Migration[]", (v): v is Migration =>
  isFileRecord(v, ["replaced"]),
);
export const Inventory = guard(
  "Inventory",
  (v: unknown): v is Inventory => isRecord(v) && typeof v.pending === "number",
);
export const Remaining = guard(
  "Remaining",
  (v: unknown): v is Remaining => isRecord(v) && typeof v.remaining === "number",
);
export const Verification = guard(
  "Verification",
  (v: unknown): v is Verification =>
    isRecord(v) && typeof v.remaining === "number" && typeof v.migrated === "number",
);
export const Config = guard(
  "Config",
  (v: unknown): v is Config => isRecord(v) && typeof v.replacement === "string",
);

// `codemod/ast-grep` in Node.js and Bun, backed by the native addon: the same
// SgRoot and SgNode methods, matchers and languages as in the engine.
//
// Roots parsed here have no semantic provider, as with `parse()` in an engine
// `forEachFile` callback: `definition()` returns null.

import { readFileSync, realpathSync } from "node:fs";
import { isAbsolute, join as joinPath, relative } from "node:path";

import { addon } from "./addon.mts";
import { context } from "./context.mts";

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object";
}

/** The `language` of `parse`'s options, when it is given. */
function languageOption(options: unknown): string | undefined {
  return isRecord(options) && typeof options.language === "string" ? options.language : undefined;
}

/**
 * `parse(lang, text)`, `parse(text, { language })`, or `parse(file, { language })`
 * with a `forEachFile` file. Without a language, it comes from the file, then
 * the `forEachFile` step's language, then the file's extension.
 */
export function parse(source: unknown, second?: unknown): unknown {
  const native = addon();
  const store = context.getStore();
  const fallback = store?.step?.language;

  if (typeof source === "string") {
    if (typeof second === "string") return native.parse(source, second);
    const language = languageOption(second) ?? fallback;
    if (!language) {
      throw new Error('parse(text) needs a language: parse(text, { language: "tsx" })');
    }
    return native.parse(language, source);
  }
  if (!isRecord(source)) {
    throw new TypeError(
      "parse() expects source text or a file object, for example parse(file) or parse(text, { language })",
    );
  }
  if (typeof source.text !== "string") {
    throw new TypeError(
      "parse(file) expects an object with a string `text`, such as the file a forEachFile callback receives",
    );
  }
  const path = typeof source.path === "string" ? source.path : undefined;
  const language =
    languageOption(second) ??
    languageOption(source) ??
    fallback ??
    (path ? (native.languageFromPath(path) ?? undefined) : undefined);
  if (!language) {
    throw new Error(
      `parse(file): no language for '${path ?? "<file>"}'; pass one with parse(file, { language })`,
    );
  }
  if (path === undefined) return native.parse(language, source.text);
  // The file's real path, and its path inside the target: what the CLI's
  // runtime gives `filename()` and `relativeFilename()`.
  const target = store?.run.target;
  const filename = real(isAbsolute(path) || !target ? path : joinPath(target, path));
  const inTarget = target ? relative(real(target), filename) : path;
  const outside = inTarget.startsWith("..") || isAbsolute(inTarget);
  return native.parse(language, source.text, filename, outside ? undefined : inTarget);
}

/** A path with links resolved; as given when it does not exist. */
function real(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return path;
  }
}

/** The same as `parse`; parsers are ready at once here. */
export function parseAsync(source: unknown, second?: unknown): unknown {
  return parse(source, second);
}

export function parseFile(language: string, path: string): unknown {
  return addon().parse(language, readFileSync(path, "utf8"), path);
}

/** The numeric id of a named node kind, for `{ kind: id }` matchers. */
export function kind(language: string, kindName: string): number {
  return addon().kind(language, kindName);
}

export function jssgTransform(): never {
  throw new Error(
    "jssgTransform() is not supported in forEachFile callbacks yet; change each file from its own callback",
  );
}

// The workspace analyses need the engine's semantic provider, which roots parsed
// here have none of. Each throws when called, so a codemod that uses one fails
// with what to do instead of reading "no callers" from a missing export.
function needsTheCli(what: string): never {
  throw new Error(
    `${what} is not available on the Node.js host yet: run the workflow with the codemod CLI, or test it with { host: "quickjs" }`,
  );
}

export function callGraph(..._args: unknown[]): never {
  return needsTheCli("callGraph()");
}
export function dataflow(..._args: unknown[]): never {
  return needsTheCli("dataflow()");
}
export function symbols(..._args: unknown[]): never {
  return needsTheCli("symbols()");
}
export function types(..._args: unknown[]): never {
  return needsTheCli("types()");
}
export function entryPoints(..._args: unknown[]): never {
  return needsTheCli("entryPoints()");
}
export function lookup(..._args: unknown[]): never {
  return needsTheCli("lookup()");
}
export function join(..._args: unknown[]): never {
  return needsTheCli("join()");
}
export function crawl(..._args: unknown[]): never {
  return needsTheCli("crawl()");
}

export default {
  parse,
  parseAsync,
  parseFile,
  kind,
  jssgTransform,
  callGraph,
  dataflow,
  symbols,
  types,
  entryPoints,
  lookup,
  join,
  crawl,
};

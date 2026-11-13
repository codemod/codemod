// `codemod:ast-grep` in Node.js and Bun, backed by the native addon: the same
// SgRoot and SgNode methods, matchers and languages as in the engine.
//
// Roots parsed here have no semantic provider, as with `parse()` in an engine
// `forEachFile` callback: `definition()` returns null.

import { readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";

import { addon } from "./addon.mjs";
import { context } from "./context.mjs";

/**
 * `parse(lang, text)`, `parse(text, { language })`, or `parse(file, { language })`
 * with a `forEachFile` file. Without a language, it comes from the file, then
 * the `forEachFile` step's language, then the file's extension.
 */
export function parse(source, second) {
  const native = addon();
  const store = context.getStore();
  const fallback = store?.step?.language;
  const languageOption = (value) =>
    value && typeof value === "object" ? (value.language ?? undefined) : undefined;

  if (typeof source === "string") {
    if (typeof second === "string") return native.parse(source, second);
    const language = languageOption(second) ?? fallback;
    if (!language) {
      throw new Error('parse(text) needs a language: parse(text, { language: "tsx" })');
    }
    return native.parse(language, source);
  }
  if (!source || typeof source !== "object") {
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
    source.language ??
    fallback ??
    (path ? native.languageFromPath(path) : undefined);
  if (!language) {
    throw new Error(
      `parse(file): no language for '${path ?? "<file>"}'; pass one with parse(file, { language })`,
    );
  }
  if (path === undefined) return native.parse(language, source.text);
  const target = store?.run?.target;
  const filename = isAbsolute(path) || !target ? path : join(target, path);
  return native.parse(language, source.text, filename, isAbsolute(path) ? undefined : path);
}

/** The same as `parse`; parsers are ready at once here. */
export function parseAsync(source, second) {
  return parse(source, second);
}

export function parseFile(language, path) {
  return addon().parse(language, readFileSync(path, "utf8"), path);
}

/** The numeric id of a named node kind, for `{ kind: id }` matchers. */
export function kind(language, kindName) {
  return addon().kind(language, kindName);
}

export function jssgTransform() {
  throw new Error(
    "jssgTransform() is not supported in forEachFile callbacks yet; change each file from its own callback",
  );
}

export default { parse, parseAsync, parseFile, kind, jssgTransform };

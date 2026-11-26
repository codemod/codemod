// Named cases for the semantic test codemods: one rule each, each passing or
// failing on its own, so one broken rule never hides the others.
//
//   import { equal, test } from "./spec.js";
//   export { run as default } from "./spec.js";
//
//   test("sources stops at a parameter", async (root) => { ... });
//
// The harness runs the codemod once on each target of its fixture. A case
// inside onFile(path, ...) runs on the target whose path ends with `path`;
// any other case runs on every target, so an entry with one target needs no
// onFile. run() answers each case's outcome as the transform's data, and the
// Rust side fails the test listing every failed case by name.

const cases = [];
let currentFile = null;

export function test(name, fn) {
  if (cases.some((c) => c.name === name)) throw new Error(`two cases are named "${name}"`);
  cases.push({ name, fn, file: currentFile });
}

export function onFile(path, register) {
  currentFile = path;
  try {
    register();
  } finally {
    currentFile = null;
  }
}

export async function run(root, options) {
  const filename = root.filename().replace(/\\/g, "/");
  const results = [];
  for (const { name, fn, file } of cases) {
    if (file && !filename.endsWith("/" + file)) {
      results.push({ name, status: "skip" });
      continue;
    }
    const start = Date.now();
    try {
      await fn(root, options);
      results.push({ name, status: "pass", ms: Date.now() - start });
    } catch (error) {
      const message = String(error?.message ?? error);
      results.push({ name, status: "fail", message, ms: Date.now() - start });
    }
  }
  return { data: results };
}

const show = (value) => (value === undefined ? "undefined" : JSON.stringify(value));

// Compared as JSON, so arrays and plain objects compare by content.
export function equal(actual, expected, what) {
  if (show(actual) !== show(expected)) {
    throw new Error(`${what ? what + ": " : ""}expected ${show(expected)}, got ${show(actual)}`);
  }
}

export function includes(list, item, what) {
  if (!list.includes(item)) {
    throw new Error(`${what ? what + ": " : ""}expected ${show(item)} in ${show(list)}`);
  }
}

export function excludes(list, item, what) {
  if (list.includes(item)) {
    throw new Error(`${what ? what + ": " : ""}did not expect ${show(item)} in ${show(list)}`);
  }
}

// Runs `fn` (sync or async) and expects it to throw an error whose message
// contains `needle`; answers the error.
export async function throws(fn, needle) {
  try {
    await fn();
  } catch (error) {
    const message = String(error?.message ?? error);
    if (!message.includes(needle)) {
      throw new Error(`expected an error containing ${show(needle)}, got: ${message}`);
    }
    return error;
  }
  throw new Error(`expected an error containing ${show(needle)}, got none`);
}

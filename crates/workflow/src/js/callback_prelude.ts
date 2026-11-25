// ---- appended by the build to every extracted callback or function step ----
//
// The build puts this after a module that defines `__make`, and strips the
// types from the whole.

import {
  checkJson as __checkJson,
  describeError as __describe,
  fileHandle as __fileHandle,
  freeze as __freeze,
} from "codemod-internal:orchestrator";
import type { FileEffect as __FileEffect } from "codemod-internal:orchestrator";

/** Defined by the build: the callback, given the outside values it uses. */
declare const __make: (captures: Record<string, unknown>) => (...args: unknown[]) => unknown;

let __cb: (...args: unknown[]) => unknown;
let __lang: string | undefined;

/** `captures` is JSON; `language` is what the step's files are in, when known. */
export function __init(captures: string, language?: string | null): void {
  __lang = language ?? undefined;
  __cb = __make(__freeze(JSON.parse(captures)));
}

/** What became of one file. */
type __FileOutcome =
  | { ok: false; error: string }
  | { ok: true; effects: __FileEffect[]; output: unknown; hasOutput: boolean };

/** Run the callback for one file; never throws. */
async function __one(path: string, text: string): Promise<__FileOutcome> {
  const handle = __fileHandle(path, text, __lang);
  let output: unknown;
  try {
    output = __plain(await __cb(handle.file));
    if (output !== undefined) __checkJson(output, "return value");
  } catch (error) {
    return { ok: false, error: __describe(error) };
  } finally {
    handle.close();
  }
  return {
    ok: true,
    effects: handle.effects,
    output: output === undefined ? null : output,
    hasOutput: output !== undefined,
  };
}

/** Run the callback for a batch of files, concurrently: `texts[i]` is the text of `paths[i]`. */
export function __runBatch(paths: string[], texts: string[]): Promise<string> {
  return Promise.all(paths.map((path, i) => __one(path, texts[i] as string))).then((outcomes) =>
    JSON.stringify(outcomes),
  );
}

/** Run a function step: call it with no arguments; never throws. */
export async function __runStep(): Promise<string> {
  try {
    let result = __plain(await __cb());
    if (result === undefined) result = null;
    __checkJson(result, "the step's result");
    return JSON.stringify({ ok: true, result });
  } catch (error) {
    const message =
      error && typeof error === "object" && "message" in error ? error.message : String(error);
    return JSON.stringify({ ok: false, error: message });
  }
}

/**
 * Class instances with `toJSON` (`codemod/deps` packages, declarations,
 * nodes and edges) into plain values, as JSON.stringify would make them.
 */
function __plain(value: unknown): unknown {
  if (value && typeof value === "object") {
    const prototype = Object.getPrototypeOf(value);
    if ("toJSON" in value && typeof value.toJSON === "function" && prototype !== Object.prototype) {
      return value.toJSON();
    }
    if (Array.isArray(value)) return value.map(__plain);
    if (prototype === Object.prototype) {
      const plain: Record<string, unknown> = {};
      // defineProperty keeps a `__proto__` key as data, where assigning it
      // would set the object's prototype.
      for (const [key, item] of Object.entries(value)) {
        Object.defineProperty(plain, key, {
          value: __plain(item),
          enumerable: true,
          writable: true,
          configurable: true,
        });
      }
      return plain;
    }
  }
  return value;
}

let __folded: unknown;

/** Start a `forEachFile` reducer's fold; `initial` is JSON. */
export function __reduceStart(initial: string): void {
  __folded = JSON.parse(initial);
}

/** Fold one file's output (as JSON) into the result; throws what the reducer throws. */
export async function __reduceOne(output: string, path: string): Promise<void> {
  __folded = await __cb(__folded, JSON.parse(output), path);
}

/** The folded result, as JSON. */
export function __reduceEnd(): string {
  const result = __folded === undefined ? null : __folded;
  __checkJson(result, "the reducer's result");
  return JSON.stringify(result);
}

/** Call the function with arguments (a JSON array): a shard function. Never throws. */
export async function __runCall(args: string): Promise<string> {
  try {
    let result = __plain(await __cb(...(JSON.parse(args) as unknown[])));
    if (result === undefined) result = null;
    __checkJson(result, "the function's result");
    return JSON.stringify({ ok: true, result });
  } catch (error) {
    return JSON.stringify({ ok: false, error: __describe(error) });
  }
}

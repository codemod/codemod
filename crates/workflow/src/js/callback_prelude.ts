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
    output = await __cb(handle.file);
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

/** Run the callback for a batch of `[path, text]` pairs (as JSON), concurrently. */
export function __runBatch(batch: string): Promise<string> {
  const files: [path: string, text: string][] = JSON.parse(batch);
  return Promise.all(files.map(([path, text]) => __one(path, text))).then((outcomes) =>
    JSON.stringify(outcomes),
  );
}

/** Run a function step: call it with no arguments; never throws. */
export async function __runStep(): Promise<string> {
  try {
    let result = await __cb();
    if (result === undefined) result = null;
    __checkJson(result, "the step's result");
    return JSON.stringify({ ok: true, result });
  } catch (error) {
    const message =
      error && typeof error === "object" && "message" in error ? error.message : String(error);
    return JSON.stringify({ ok: false, error: message });
  }
}

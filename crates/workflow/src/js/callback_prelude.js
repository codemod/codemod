// ---- appended by the build to every extracted callback or function step ----

import {
  checkJson as __checkJson,
  describeError as __describe,
  fileHandle as __fileHandle,
  freeze as __freeze,
} from "codemod-internal:orchestrator";

let __cb;
let __lang;

export function __init(captures, language) {
  __lang = language ?? undefined;
  __cb = __make(__freeze(JSON.parse(captures)));
}

/** Run the callback for one file; never throws. */
async function __one(path, text) {
  const handle = __fileHandle(path, text, __lang);
  let output;
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

/** Run the callback for a batch of `[path, text]` pairs, concurrently. */
export function __runBatch(batch) {
  return Promise.all(JSON.parse(batch).map(([path, text]) => __one(path, text))).then((outcomes) =>
    JSON.stringify(outcomes),
  );
}

/** Run a function step: call it with no arguments; never throws. */
export async function __runStep() {
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

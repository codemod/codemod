/**
 * `@codemod.com/jssg-napi`: loads the napi-rs addon built from
 * `crates/jssg-napi` (current platform only) and provides the in-process
 * transform engine pieces the orchestration `node` runtime builds on.
 *
 * - `bindingPath()` / `isAvailable()` / `loadBinding()`: the addon, from
 *   `CODEMOD_JSSG_NAPI` or `./jssg-napi.<platform>-<arch>.node`
 *   (`pnpm --filter @codemod.com/jssg-napi build`).
 * - `runTransform(targetDir, params, call)`: runs one transform call as the
 *   only one in flight in this process, with `write()` / `jssgTransform()`
 *   staged (never written) and contained to `targetDir`.
 * - `currentTransform()`: the running call's `{ targetDir, params }`
 *   (used by `jssgTransform()` in `./ast-grep.js`).
 */
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

/** Where the addon is loaded from. */
export function bindingPath(env = process.env) {
  return (
    env.CODEMOD_JSSG_NAPI ??
    fileURLToPath(new URL(`./jssg-napi.${process.platform}-${process.arch}.node`, import.meta.url))
  );
}

export function isAvailable(env = process.env) {
  return existsSync(bindingPath(env));
}

let binding;

/** The addon's exports (loaded once). */
export function loadBinding() {
  binding ??= require(bindingPath());
  return binding;
}

let current;
let tail = Promise.resolve();

/** The transform call running now, if any. */
export function currentTransform() {
  return current;
}

/**
 * Run `call` (a transform invocation) once every earlier call has settled.
 * Resolves with its awaited result and the changes it staged; rejects with
 * what it threw (staging is discarded).
 */
export async function runTransform(targetDir, params, call) {
  const previous = tail;
  let release;
  tail = new Promise((resolve) => {
    release = resolve;
  });
  await previous;
  const native = loadBinding();
  native.engineBegin(targetDir);
  current = { targetDir, params };
  let ended = false;
  try {
    const result = await call();
    ended = true;
    return { result, staged: native.engineEnd() };
  } finally {
    if (!ended) native.engineEnd();
    current = undefined;
    release();
  }
}

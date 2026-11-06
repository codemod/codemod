/**
 * The `codemod:ast-grep` module for transforms running in Node: the same
 * exports as the QuickJS sandbox module (`crates/codemod-sandbox/src/ast_grep/mod.rs`)
 * over the napi addon. `jssgTransform()` stages its result through the
 * running transform's context instead of writing.
 */
import { readFileSync } from "node:fs";
import { currentTransform, loadBinding } from "./index.js";

const native = loadBinding();

export const { SgRoot, SgNode, parse, parseAsync, parseFile, kind } = native;

/**
 * `jssgTransform(transformFn, pathToFile, language)`: parse another file in
 * the target directory, run `transformFn` on it, and stage the result as a
 * secondary edit. Resolves with the new content, or `null` when unmodified.
 * Outside a running transform it is a no-op resolving to `null`, like the
 * in-memory QuickJS engine.
 */
export function jssgTransform(transformFn, pathToFile, language) {
  const context = currentTransform();
  if (context === undefined) return Promise.resolve(null);
  native.engineCheckWithinTarget(pathToFile, "jssgTransform()");
  let content;
  try {
    content = readFileSync(pathToFile, "utf8");
  } catch (error) {
    throw new Error(`Failed to read file '${pathToFile}': ${error.message}`);
  }
  const root = native.engineParseWithPath(language, content, pathToFile, context.targetDir);
  let languageName = "";
  try {
    languageName = native.engineLanguageName(language);
  } catch {
    // unreachable: the language just parsed
  }
  const value = transformFn(root, {
    params: context.params,
    language: languageName,
    matches: undefined,
    matrixValues: undefined,
    dryRun: false,
    targetDir: context.targetDir,
  });
  return (async () => {
    let result;
    try {
      result = await value;
    } catch (error) {
      throw new Error(`Transform failed: ${error instanceof Error ? error.message : error}`);
    }
    // The sandbox's result rules, in Rust: staged as a secondary change.
    return native.engineStageResult(pathToFile, root, result);
  })();
}

export default { SgRoot, SgNode, parse, parseAsync, parseFile, kind, jssgTransform };

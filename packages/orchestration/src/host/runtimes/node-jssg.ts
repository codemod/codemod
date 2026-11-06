/**
 * The `JssgEngine` of the `node` runtime: AST bindings from the napi addon
 * (`@codemod.com/jssg-napi`, built from `crates/jssg-napi`), artifacts
 * evaluated as ES modules in this process (`@codemod.com/jssg-napi/loader`:
 * in-memory module hooks, `codemod:ast-grep` resolved to the addon-backed
 * module, one evaluation per artifact per run).
 */
import { loadBinding, runTransform, type NativeSelector } from "@codemod.com/jssg-napi";
import { importArtifact } from "@codemod.com/jssg-napi/loader";
import type { Host } from "../../core/host.ts";
import type { JssgEngine } from "../../execution/jssg-inprocess.ts";

/**
 * `base` (the Node host) plus the addon's native file selection
 * (`Host.selectFiles`, `Host.fs.readSources`, `crates/orchestration-walker`):
 * the same files, contents, and error messages as the TypeScript walker
 * over `base` (`tests/files-native.test.ts`), without interpreting the walk.
 */
export function createNapiSelectHost(base: Host): Host {
  const native = loadBinding();
  return {
    ...base,
    fs: {
      ...base.fs,
      // The Node host's `readText(fatal)` decodes with `TextDecoder`, which drops a BOM.
      readSources: (root, paths) =>
        native.readSources(root, [...paths], true).map((content) => content ?? undefined),
    },
    selectFiles: (request) => native.selectFiles(JSON.stringify(request)),
  };
}

export function createNapiJssgEngine(): JssgEngine<object> {
  const native = loadBinding();
  return {
    loadTransform: (artifact, runId) => importArtifact(artifact, runId),
    languageName: (language) => native.engineLanguageName(language),
    compileSelector: (language, selector) => new native.Selector(language, selector),
    parseBatch: (language, targetRoot, files, selector) =>
      native.engineParseBatch(
        language,
        targetRoot,
        files.map((file) => file.path),
        files.map((file) => file.content),
        (selector as NativeSelector | undefined) ?? null,
      ),
    runTransform: (targetRoot, params, call) => runTransform(targetRoot, params, call),
    renameTo: (root) => native.engineRenameTo(root),
    source: (root) => native.engineSource(root),
  };
}

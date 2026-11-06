/**
 * Evaluates bundled JSSG transform artifacts as ES modules in this Node
 * process. Synchronous module hooks (`node:module` `registerHooks`, installed
 * once on first use) serve each artifact's source from memory under
 * `codemod-artifact:<runId>/<name>-<hash>.mjs` and resolve its imports:
 *
 * - `codemod:ast-grep` -> `./ast-grep.js` (the napi-backed module);
 * - any other `codemod:*` module -> an import error naming the module as
 *   unsupported in runtime 'node';
 * - everything else (Node built-ins a bundle left external) -> Node.
 *
 * The run id is part of the URL, so a module is evaluated once per run and
 * runs never share module state. Node never unloads modules: a long-lived
 * process keeps every evaluated artifact.
 */
import { createHash } from "node:crypto";
import { registerHooks } from "node:module";

const SCHEME = "codemod-artifact:";
const AST_GREP = new URL("./ast-grep.js", import.meta.url).href;
const sources = new Map();
let installed = false;

function install() {
  if (installed) return;
  installed = true;
  registerHooks({
    resolve(specifier, context, nextResolve) {
      if (specifier === "codemod:ast-grep") return { url: AST_GREP, shortCircuit: true };
      if (specifier.startsWith("codemod:")) {
        throw new Error(`module '${specifier}' is unsupported in runtime 'node'`);
      }
      if (specifier.startsWith(SCHEME)) return { url: specifier, shortCircuit: true };
      return nextResolve(specifier, context);
    },
    load(url, context, nextLoad) {
      if (url.startsWith(SCHEME)) {
        const source = sources.get(url);
        if (source === undefined) throw new Error(`unknown transform artifact ${url}`);
        return { format: "module", source, shortCircuit: true };
      }
      return nextLoad(url, context);
    },
  });
}

/**
 * Verify `artifact.source` against `artifact.hash` (as the bridge does),
 * evaluate it once for `runId`, and return its default export.
 */
export async function importArtifact(artifact, runId) {
  const actual = createHash("sha256").update(artifact.source).digest("hex");
  if (actual !== artifact.hash) {
    throw new Error(
      `JSSG transform '${artifact.name}' source hashes to ${actual}, not the recorded ${artifact.hash}`,
    );
  }
  install();
  const stem = artifact.name.replace(/[^A-Za-z0-9._-]/gu, "_");
  const url = `${SCHEME}${encodeURIComponent(runId)}/${stem}-${artifact.hash}.mjs`;
  sources.set(url, artifact.source);
  try {
    const namespace = await import(url);
    return namespace.default;
  } finally {
    // Node caches the evaluated module by URL; the text is no longer needed.
    sources.delete(url);
  }
}

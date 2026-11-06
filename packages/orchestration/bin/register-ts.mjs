// Synchronous module hooks (`node:module` `registerHooks`) that run every
// `.ts` / `.mts` / `.cts` file with full TypeScript support (Node's
// `--experimental-transform-types` semantics: enums, parameter properties,
// namespaces), in this process, without any Node flag. Importing this module
// installs them; `bin/codemod-workflow.mjs` does so before loading the CLI,
// and `node --import ./bin/register-ts.mjs file.ts` works too.
//
// - Outside `node_modules`, Node's default loader picks the format (package
//   `type`, extension, syntax detection) and these hooks strip the types.
// - Under `node_modules`, where Node's own type stripping refuses to run
//   (ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING) and where this package
//   lives once installed, the extension picks the format (`.cts` CommonJS,
//   anything else ESM).
//
// Stripped output is cached on disk by content hash (`ts/` under the build
// cache: `$CODEMOD_WORKFLOW_CACHE` or `<tmpdir>/codemod-workflow-cache-<uid>`,
// the same root and privacy rule as `src/bundle/cache.ts`), so a warm start
// never loads Node's TypeScript transformer (~30 ms).
//
// Needs Node >= 22.15 (`registerHooks`, `stripTypeScriptTypes`); the
// repository requires Node 24.
import { createHash } from "node:crypto";
import { lstatSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const TS = /\.[cm]?ts$/u;
const NODE_MODULES = /\/node_modules\//u;

/** `src/bundle/cache.ts` `cacheRoot`. */
export function cacheRoot(env = process.env) {
  const configured = env.CODEMOD_WORKFLOW_CACHE;
  if (configured !== undefined && configured !== "") return configured;
  return join(tmpdir(), `codemod-workflow-cache-${process.getuid?.() ?? "user"}`);
}

/** `src/bundle/cache.ts` `privateDirectory`. */
export function privateDirectory(name, root = cacheRoot()) {
  const directory = join(root, name);
  try {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    for (const path of [root, directory]) {
      const stat = lstatSync(path);
      if (!stat.isDirectory() || (stat.mode & 0o022) !== 0) return undefined;
      if (process.getuid !== undefined && stat.uid !== process.getuid()) return undefined;
    }
    return directory;
  } catch {
    return undefined;
  }
}

let stripCache;
function strip(source, url) {
  stripCache ??= privateDirectory("ts") ?? null;
  const key = createHash("sha256")
    .update(`${process.version}\0${url}\0`)
    .update(source)
    .digest("hex");
  const entry = stripCache === null ? undefined : join(stripCache, `${key}.js`);
  if (entry !== undefined) {
    try {
      return readFileSync(entry, "utf8");
    } catch {
      // Not cached yet.
    }
  }
  const emitWarning = process.emitWarning;
  // `stripTypeScriptTypes` warns once that it is experimental; this runner
  // used to pass `--disable-warning=ExperimentalWarning` for the same reason.
  process.emitWarning = function (warning, ...rest) {
    const type = typeof rest[0] === "string" ? rest[0] : rest[0]?.type;
    if (type === "ExperimentalWarning" && String(warning).includes("stripTypeScriptTypes")) return;
    return emitWarning.call(process, warning, ...rest);
  };
  let stripped;
  try {
    stripped = stripTypeScriptTypes(source, { mode: "transform", sourceUrl: url });
  } finally {
    process.emitWarning = emitWarning;
  }
  if (entry !== undefined) {
    try {
      const temporary = `${entry}.${process.pid}.${Date.now()}.tmp`;
      writeFileSync(temporary, stripped, { mode: 0o600 });
      renameSync(temporary, entry);
    } catch {
      // Uncached: the next start strips again.
    }
  }
  return stripped;
}

registerHooks({
  load(url, context, nextLoad) {
    if (!url.startsWith("file:") || !TS.test(new URL(url).pathname)) {
      return nextLoad(url, context);
    }
    if (NODE_MODULES.test(url)) {
      const source = readFileSync(fileURLToPath(url), "utf8");
      return {
        format: url.endsWith(".cts") ? "commonjs" : "module",
        source: strip(source, url),
        shortCircuit: true,
      };
    }
    const loaded = nextLoad(url, context);
    if (loaded.format !== "module-typescript" && loaded.format !== "commonjs-typescript") {
      return loaded;
    }
    const raw = loaded.source ?? readFileSync(fileURLToPath(url));
    const source = typeof raw === "string" ? raw : Buffer.from(raw).toString("utf8");
    return {
      ...loaded,
      format: loaded.format === "module-typescript" ? "module" : "commonjs",
      source: strip(source, url),
    };
  },
});

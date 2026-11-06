// Synchronous module hooks (`node:module` `registerHooks`) that run every
// `.ts` / `.mts` / `.cts` file with full TypeScript support (Node's
// `--experimental-transform-types` semantics: enums, parameter properties,
// namespaces), in this process, without any Node flag. Importing this module
// installs them; `bin/codemod-workflow.mjs` does so before loading the CLI,
// and `node --import ./bin/register-ts.mjs file.ts` works too.
//
// The format follows Node's rules (`formatOf`), and `node_modules` is
// covered too: Node's own type stripping refuses it
// (ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING), and that is exactly where
// this package lives once installed.
//
// Stripped output is cached on disk by content hash (`ts/` under the build
// cache: `$CODEMOD_WORKFLOW_CACHE` or `<tmpdir>/codemod-workflow-cache-<uid>`,
// the same root and privacy rule as `src/bundle/cache.ts`), so a warm start
// never loads Node's TypeScript transformer (~30 ms).
//
// Needs Node >= 22.15 (`registerHooks`, `stripTypeScriptTypes`,
// `findPackageJSON`); the repository requires Node 24.
import { createHash } from "node:crypto";
import { lstatSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { findPackageJSON, registerHooks, stripTypeScriptTypes } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { compileFunction } from "node:vm";

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
  const isPrivate = (path) => {
    const stat = lstatSync(path);
    if (!stat.isDirectory() || (stat.mode & 0o022) !== 0) return false;
    return process.getuid === undefined || stat.uid === process.getuid();
  };
  try {
    // The root is checked before anything is created inside it.
    mkdirSync(root, { recursive: true, mode: 0o700 });
    if (!isPrivate(root)) return undefined;
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    return isPrivate(directory) ? directory : undefined;
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

// The syntax errors Node's CommonJS-or-ESM detection takes as ESM syntax.
const ESM_SYNTAX = [
  "Cannot use import statement outside a module",
  "Unexpected token 'export'",
  "Cannot use 'import.meta' outside a module",
  "await is only valid in async functions and the top level bodies of modules",
];

/** Node's syntax detection for a file in no `type`d package, on stripped JavaScript. */
function detectFormat(source) {
  try {
    compileFunction(source, ["exports", "require", "module", "__filename", "__dirname"]);
    return "commonjs";
  } catch (error) {
    return error instanceof SyntaxError && ESM_SYNTAX.some((m) => error.message.includes(m))
      ? "module"
      : "commonjs";
  }
}

const packageTypes = new Map();
/** `type` of the nearest `package.json` (`module`, `commonjs`, or undefined). */
function packageType(url) {
  const manifest = findPackageJSON(url);
  if (manifest === undefined) return undefined;
  if (!packageTypes.has(manifest)) {
    let type;
    try {
      type = JSON.parse(readFileSync(manifest, "utf8")).type;
    } catch {
      type = undefined;
    }
    packageTypes.set(manifest, type === "module" || type === "commonjs" ? type : undefined);
  }
  return packageTypes.get(manifest);
}

/**
 * Node's rules for TypeScript files: `.mts` ESM, `.cts` CommonJS, `.ts` by
 * the nearest `package.json` `type`, else by syntax. Under `node_modules`,
 * Node gives typeless `.ts` no detection; ESM, as before.
 */
function formatOf(url, source) {
  if (url.endsWith(".mts")) return "module";
  if (url.endsWith(".cts")) return "commonjs";
  const type = packageType(url);
  if (type !== undefined) return type;
  return NODE_MODULES.test(url) ? "module" : detectFormat(source);
}

// The format is decided here rather than by Node's default loader, because
// for a `.ts` file outside a `type`d package Node detects it by stripping
// the file in strip-only mode, which loads the stripper (defeating the cache)
// and rejects enums and parameter properties.
registerHooks({
  load(url, context, nextLoad) {
    if (!url.startsWith("file:") || !TS.test(new URL(url).pathname)) {
      return nextLoad(url, context);
    }
    const source = strip(readFileSync(fileURLToPath(url), "utf8"), url);
    return { format: formatOf(url, source), source, shortCircuit: true };
  },
});

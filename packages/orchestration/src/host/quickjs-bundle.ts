/**
 * `codemod-workflow build <workflow.ts> --runtime=quickjs -o <file>`: one
 * self-contained ES module for the `quickjs` runtime, built ahead of time in
 * Node with esbuild. It contains:
 *
 * - the portable control plane (`src/portable.ts` closure: authoring,
 *   `run()`, replay, history, scheduling, the JSSG + shell execution path)
 *   and the QuickJS entry (`quickjs/entry.ts`: Host over the Rust
 *   primitives, in-context JSSG dispatch);
 * - the workflow module graph, every module that calls `jssg(` rewritten by
 *   the build step (`bundle/build.ts`) so each inline transform is an
 *   `ArtifactRef`, so command identities and history are the transforms'
 *   content hashes;
 * - every transform artifact's source, verbatim, keyed by hash (the host
 *   registers each as a module and evaluates it once per run, on first use).
 *
 * `@codemod.com/orchestration` (and `/portable`) resolve to `src/portable.ts`;
 * `node:*` imports fail the build. `codemod:*` modules stay external: the
 * host provides them (transforms, separate artifacts, import them).
 *
 * Async functions and generators are lowered to generators so continuations
 * go through `Promise.prototype.then`, which carries the orchestration async
 * context in QuickJS (`quickjs/async-context.ts`).
 */
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, extname, isAbsolute, join, resolve, sep } from "node:path";
// esbuild and TypeScript load only when a bundle is built (the build step
// loads TypeScript on first use), so a cache hit costs file hashing only.
import type { Plugin } from "esbuild";
import {
  buildModule,
  buildToolchain,
  type BuiltModule,
  type JssgArtifact,
} from "../bundle/build.ts";
import {
  atomicWrite,
  cacheRoot,
  fileHash,
  inputsUnchanged,
  privateDirectory,
  sha256,
} from "../bundle/cache.ts";

export interface QuickjsBundle {
  /** The ES module text. */
  code: string;
  /** SHA-256 (hex) of `code`. */
  hash: string;
  artifacts: JssgArtifact[];
  /** Absolute paths of every source file that went into the bundle. */
  inputs: string[];
}

const SRC = resolve(import.meta.dirname, "..");
const PORTABLE = join(SRC, "portable.ts");
const ENTRY = join(SRC, "host", "quickjs", "entry.ts");
const ARTIFACTS = "__CODEMOD_QUICKJS_ARTIFACTS__";
const ENTRY_NAME = "quickjs-entry.js";
const PACKAGE = "@codemod.com/orchestration";

type Loader = "ts" | "tsx" | "js" | "jsx";
const loaderFor = (file: string): Loader => {
  const extension = extname(file);
  if (extension === ".tsx") return "tsx";
  if (extension === ".jsx") return "jsx";
  if ([".js", ".mjs", ".cjs"].includes(extension)) return "js";
  return "ts";
};

function plugin(
  artifacts: Map<string, JssgArtifact>,
  transformInputs: Set<string>,
  buildModule: (source: string, file: string) => BuiltModule,
): Plugin {
  return {
    name: "codemod-quickjs",
    setup(build) {
      // esbuild filters are Go regular expressions: no `u` flag.
      // The package's Node entry, reached by a relative import from inside
      // this package (tests, demos), is the portable API as well.
      build.onResolve({ filter: /(^|\/)index(\.ts)?$/ }, (args) => {
        if (!args.path.startsWith(".") && !isAbsolute(args.path)) return undefined;
        const target = resolve(args.resolveDir, args.path);
        return target === join(SRC, "index") || target === join(SRC, "index.ts")
          ? { path: PORTABLE }
          : undefined;
      });
      build.onResolve({ filter: /^@codemod\.com\/orchestration(\/.*)?$/ }, (args) => {
        if (args.path === PACKAGE || args.path === `${PACKAGE}/portable`) {
          return { path: PORTABLE };
        }
        return {
          errors: [
            {
              text: `'${args.path}' is not available in the quickjs runtime; import '${PACKAGE}' (the portable API)`,
            },
          ],
        };
      });
      build.onResolve({ filter: /^node:/ }, (args) => ({
        errors: [
          {
            text: `'${args.path}' (imported by ${args.importer}) is a Node module; the quickjs runtime has no Node APIs`,
          },
        ],
      }));
      // Provided by the host (the same built-ins transforms import).
      build.onResolve({ filter: /^codemod:/ }, (args) => ({ path: args.path, external: true }));
      build.onLoad({ filter: /\.(?:[cm]?[jt]s|[jt]sx)$/ }, (args) => {
        // The package's own sources define `jssg`; they never call it.
        if (args.path.startsWith(SRC + sep)) return undefined;
        const text = readFileSync(args.path, "utf8");
        if (!text.includes("jssg(")) return undefined;
        const built = buildModule(text, args.path);
        for (const artifact of built.artifacts) artifacts.set(artifact.hash, artifact);
        for (const input of built.inputs ?? []) transformInputs.add(input);
        return {
          contents: built.source,
          loader: loaderFor(args.path),
          resolveDir: dirname(args.path),
        };
      });
    },
  };
}

/** Build the bundle for `workflow` (absolute or relative to the process cwd). */
export async function buildQuickjsBundle(workflow: string): Promise<QuickjsBundle> {
  const file = resolve(workflow);
  const { build } = await import("esbuild");
  const artifacts = new Map<string, JssgArtifact>();
  const transformInputs = new Set<string>();
  // The QuickJS entry first, so the host and the async context are installed
  // before the workflow's modules evaluate.
  const entry = [
    `import { runBundled } from ${JSON.stringify(ENTRY)};`,
    `import * as workflow from ${JSON.stringify(file)};`,
    `export function main(request) { return runBundled(workflow, ${ARTIFACTS}, request); }`,
    "",
  ].join("\n");
  const result = await build({
    stdin: { contents: entry, resolveDir: dirname(file), sourcefile: ENTRY_NAME, loader: "js" },
    absWorkingDir: dirname(file),
    bundle: true,
    write: false,
    metafile: true,
    format: "esm",
    platform: "neutral",
    mainFields: ["module", "main"],
    target: "es2022",
    supported: { "async-await": false, "async-generator": false, "for-await": false },
    legalComments: "none",
    logLevel: "silent",
    charset: "utf8",
    plugins: [plugin(artifacts, transformInputs, buildModule)],
  }).catch((error: unknown) => {
    const messages = (
      error as { errors?: { text: string; location?: { file: string; line: number } | null }[] }
    ).errors;
    const detail =
      messages
        ?.map((m) => (m.location ? `${m.location.file}:${m.location.line}: ${m.text}` : m.text))
        .join("\n") ?? (error as Error).message;
    throw new Error(`failed to build the quickjs bundle for ${file}:\n${detail}`);
  });
  const sorted = [...artifacts.values()].sort((a, b) => (a.hash < b.hash ? -1 : 1));
  const table = Object.fromEntries(
    sorted.map((artifact) => [artifact.hash, { name: artifact.name, source: artifact.source }]),
  );
  const code = `const ${ARTIFACTS} = JSON.parse(${JSON.stringify(JSON.stringify(table))});\n${result.outputFiles[0]!.text}`;
  const hash = sha256(code);
  const inputs = Object.keys(result.metafile.inputs)
    // The generated entry (stdin) is a function of the workflow path and this builder.
    .filter((input) => !input.startsWith("<stdin>") && !input.endsWith(ENTRY_NAME))
    // Files bundled into transform artifacts (helpers the rewritten module no longer imports).
    .concat([...transformInputs])
    .map((input) => (isAbsolute(input) ? input : resolve(dirname(file), input)));
  return { code, hash, artifacts: sorted, inputs: [...new Set(inputs)].sort() };
}

/**
 * Identity of this builder (this file and the build step's toolchain): a
 * cached bundle is reused only by the same build code.
 */
const BUILDER = sha256(readFileSync(join(SRC, "host", "quickjs-bundle.ts")), buildToolchain());

interface CacheManifest {
  builder: string;
  workflow: string;
  bundle: string;
  hash: string;
  inputs: Record<string, string>;
}

/**
 * Default cache: `quickjs/` under the build cache (`bundle/cache.ts`:
 * `$CODEMOD_WORKFLOW_CACHE` or `<tmpdir>/codemod-workflow-cache-<uid>`).
 * Throws when that directory is not private to this user.
 */
export function defaultBundleCache(env: Record<string, string | undefined> = process.env): string {
  const root = cacheRoot(env);
  const directory = privateDirectory("quickjs", root);
  if (directory === undefined) {
    throw new Error(
      `the workflow cache ${join(root, "quickjs")} is not a directory private to this user; fix its owner and permissions or set CODEMOD_WORKFLOW_CACHE`,
    );
  }
  return directory;
}

export interface CachedBundle {
  /** Absolute path of the bundle file. */
  path: string;
  hash: string;
  /** False when the bundle was built by this call. */
  cached: boolean;
}

/**
 * The bundle for `workflow` from `cache`, rebuilt only when any input file
 * (the workflow graph, the orchestration sources, bundled packages) or this
 * builder changed since it was cached. Bundles are stored by content hash.
 */
export async function cachedQuickjsBundle(
  workflow: string,
  cache: string = defaultBundleCache(),
): Promise<CachedBundle> {
  const file = resolve(workflow);
  const key = sha256(BUILDER, file);
  const manifestPath = join(cache, `${key}.json`);
  if (existsSync(manifestPath)) {
    try {
      const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as CacheManifest;
      const bundle = join(cache, manifest.bundle);
      if (
        manifest.builder === BUILDER &&
        manifest.workflow === file &&
        fileHash(bundle) === manifest.hash &&
        inputsUnchanged(manifest.inputs)
      ) {
        return { path: bundle, hash: manifest.hash, cached: true };
      }
    } catch {
      // A corrupt manifest is rebuilt.
    }
  }
  const built = await buildQuickjsBundle(file);
  mkdirSync(cache, { recursive: true });
  const bundleName = `${built.hash}.mjs`;
  const bundlePath = join(cache, bundleName);
  if (fileHash(bundlePath) !== built.hash) atomicWrite(bundlePath, built.code);
  const manifest: CacheManifest = {
    builder: BUILDER,
    workflow: file,
    bundle: bundleName,
    hash: built.hash,
    inputs: Object.fromEntries(built.inputs.map((input) => [input, fileHash(input) ?? ""])),
  };
  atomicWrite(manifestPath, JSON.stringify(manifest));
  return { path: bundlePath, hash: built.hash, cached: false };
}

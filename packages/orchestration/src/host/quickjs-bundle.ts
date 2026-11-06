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
 *   `ArtifactRef`, exactly as `loadWorkflow` does for the Node runtimes, so
 *   command identities and history match the other runtimes;
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
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, isAbsolute, join, resolve, sep } from "node:path";
import { build, type Plugin } from "esbuild";
import { buildModule, type JssgArtifact } from "../bundle/build.ts";

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

function plugin(artifacts: Map<string, JssgArtifact>, transformInputs: Set<string>): Plugin {
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
    plugins: [plugin(artifacts, transformInputs)],
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
  const hash = createHash("sha256").update(code).digest("hex");
  const inputs = Object.keys(result.metafile.inputs)
    // The generated entry (stdin) is a function of the workflow path and this builder.
    .filter((input) => !input.startsWith("<stdin>") && !input.endsWith(ENTRY_NAME))
    // Files bundled into transform artifacts (helpers the rewritten module no longer imports).
    .concat([...transformInputs])
    .map((input) => (isAbsolute(input) ? input : resolve(dirname(file), input)));
  return { code, hash, artifacts: sorted, inputs: [...new Set(inputs)].sort() };
}

const fileHash = (path: string): string | undefined => {
  try {
    return createHash("sha256").update(readFileSync(path)).digest("hex");
  } catch {
    return undefined;
  }
};

/** Identity of this builder itself: a cached bundle is reused only by the same build code. */
const BUILDER = createHash("sha256")
  .update(readFileSync(join(SRC, "host", "quickjs-bundle.ts")))
  .update(readFileSync(join(SRC, "bundle", "build.ts")))
  .digest("hex");

interface CacheManifest {
  builder: string;
  workflow: string;
  bundle: string;
  hash: string;
  inputs: Record<string, string>;
}

/** Default cache: `$CODEMOD_WORKFLOW_CACHE` or `<tmpdir>/codemod-workflow-quickjs`. */
export function defaultBundleCache(env: Record<string, string | undefined> = process.env): string {
  return env.CODEMOD_WORKFLOW_CACHE ?? join(tmpdir(), "codemod-workflow-quickjs");
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
  const key = createHash("sha256").update(BUILDER).update("\0").update(file).digest("hex");
  const manifestPath = join(cache, `${key}.json`);
  if (existsSync(manifestPath)) {
    try {
      const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as CacheManifest;
      const bundle = join(cache, manifest.bundle);
      if (
        manifest.builder === BUILDER &&
        manifest.workflow === file &&
        existsSync(bundle) &&
        Object.entries(manifest.inputs).every(([input, hash]) => fileHash(input) === hash)
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
  if (!existsSync(bundlePath)) atomicWrite(bundlePath, built.code);
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

function atomicWrite(path: string, content: string): void {
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`;
  writeFileSync(temporary, content);
  renameSync(temporary, path);
}

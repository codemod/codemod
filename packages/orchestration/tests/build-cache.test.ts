/**
 * Startup caches (`src/bundle/cache.ts`): the rewritten-module cache of
 * `loadWorkflow`, the type-stripping cache of `bin/register-ts.mjs`, and the
 * quickjs bundle cache. A warm load must not need TypeScript, and every
 * change to a module, a bundled helper, or a cache entry must rebuild.
 *
 * `loadWorkflow` and the module hooks are exercised in child processes:
 * Node caches modules per URL for the life of a process, and vitest's own
 * loader would bypass the hooks.
 */
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { afterAll, beforeEach, describe, expect, it } from "vitest";
import { buildFile } from "../src/index.ts";
import { cachedQuickjsBundle } from "../src/host/quickjs-bundle.ts";

const register = resolve(import.meta.dirname, "../bin/register-ts.mjs");
const buildUrl = pathToFileURL(resolve(import.meta.dirname, "../src/bundle/build.ts")).href;
const index = JSON.stringify(resolve(import.meta.dirname, "../src/index.ts"));

const scratch = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-build-cache-")));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

let dir: string;
let cache: string;
let workflow: string;
let helper: string;
beforeEach((context) => {
  dir = join(scratch, context.task.id);
  cache = join(dir, "cache");
  workflow = join(dir, "workflow.ts");
  helper = join(dir, "helper.ts");
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    helper,
    `export const rewrite = (text: string): string => text.replace("a", "one");\n`,
  );
  writeFileSync(
    workflow,
    `import { jssg } from ${index};
import { rewrite } from "./helper.ts";
export default jssg({
  name: "rewrite",
  language: "typescript",
  transform(root) {
    return rewrite(root.root().text());
  },
});
`,
  );
});

interface Loaded {
  artifacts: { name: string; hash: string; source: string }[];
  /** Whether the TypeScript compiler was loaded in the process. */
  typescript: boolean;
}

/** `loadWorkflow(workflow)` in a fresh Node process with the bin's module hooks. */
function load(env: Record<string, string> = {}): Loaded {
  const probe = `
import { createRequire } from "node:module";
const { loadWorkflow } = await import(${JSON.stringify(buildUrl)});
const { artifacts } = await loadWorkflow(process.argv[1]);
const loaded = Object.keys(createRequire(process.cwd() + "/").cache);
process.stdout.write(JSON.stringify({
  artifacts: [...artifacts.values()].map(({ name, hash, source }) => ({ name, hash, source })),
  typescript: loaded.some((path) => /[\\\\/]typescript[\\\\/]lib[\\\\/]typescript\\.js$/.test(path)),
}));
`;
  const result = spawnSync(
    process.execPath,
    ["--import", register, "--input-type=module", "--eval", probe, workflow],
    { encoding: "utf8", env: { ...process.env, CODEMOD_WORKFLOW_CACHE: cache, ...env } },
  );
  expect(result.status, result.stderr).toBe(0);
  expect(result.stderr).toBe("");
  return JSON.parse(result.stdout) as Loaded;
}

const expected = () =>
  buildFile(workflow).artifacts.map(({ name, hash, source }) => ({ name, hash, source }));

const entries = () => readdirSync(join(cache, "modules")).filter((name) => name.endsWith(".json"));

describe("loadWorkflow's rewritten-module cache", () => {
  it("builds once, then serves the same artifacts without loading TypeScript", () => {
    const cold = load();
    expect(cold.typescript).toBe(true);
    expect(cold.artifacts).toEqual(expected());
    expect(entries()).toHaveLength(1);

    const warm = load();
    expect(warm.typescript).toBe(false);
    expect(warm.artifacts).toEqual(cold.artifacts);
  });

  it("rebuilds when a helper bundled into a transform changes", () => {
    const before = load();
    writeFileSync(
      helper,
      `export const rewrite = (text: string): string => text.replace("a", "two");\n`,
    );
    const after = load();
    expect(after.typescript).toBe(true);
    expect(after.artifacts).toEqual(expected());
    expect(after.artifacts[0]!.hash).not.toBe(before.artifacts[0]!.hash);
    expect(after.artifacts[0]!.source).toContain('"two"');
    expect(load().typescript).toBe(false);
  });

  it("rebuilds when the workflow module changes", () => {
    const before = load();
    writeFileSync(
      workflow,
      readFileSync(workflow, "utf8").replace("root.root().text()", "root.root().text().trim()"),
    );
    const after = load();
    expect(after.typescript).toBe(true);
    expect(after.artifacts).toEqual(expected());
    expect(after.artifacts[0]!.hash).not.toBe(before.artifacts[0]!.hash);
  });

  it("ignores an entry whose artifact no longer matches its hash", () => {
    const cold = load();
    const [entry] = entries();
    const path = join(cache, "modules", entry!);
    const data = JSON.parse(readFileSync(path, "utf8")) as Loaded;
    data.artifacts[0]!.source = "export default () => 'tampered';\n";
    writeFileSync(path, JSON.stringify(data));
    const reloaded = load();
    expect(reloaded.typescript).toBe(true);
    expect(reloaded.artifacts).toEqual(cold.artifacts);
  });

  it("does not use a cache directory other users can write to", () => {
    mkdirSync(cache, { recursive: true });
    chmodSync(cache, 0o777);
    try {
      expect(load().typescript).toBe(true);
      expect(load().typescript).toBe(true);
      expect(readdirSync(cache)).not.toContain("modules");
    } finally {
      chmodSync(cache, 0o700);
    }
  });
});

describe("bin/register-ts.mjs", () => {
  const runTs = (file: string) =>
    spawnSync(process.execPath, ["--import", register, file], {
      encoding: "utf8",
      env: { ...process.env, CODEMOD_WORKFLOW_CACHE: cache },
    });

  it("runs transform-only TypeScript in this process, caches the output, and stays quiet", () => {
    const file = join(dir, "syntax.ts");
    writeFileSync(
      file,
      `enum Color { Red = "red" }
class Box { constructor(readonly value: number) {} }
console.log(Color.Red, new Box(1).value);
`,
    );
    const first = runTs(file);
    expect(first.status, first.stderr).toBe(0);
    expect(first.stdout).toBe("red 1\n");
    expect(first.stderr).toBe("");
    const stripped = readdirSync(join(cache, "ts"));
    expect(stripped.length).toBeGreaterThan(0);

    const second = runTs(file);
    expect(second.stdout).toBe("red 1\n");
    expect(readdirSync(join(cache, "ts"))).toEqual(stripped);

    writeFileSync(file, readFileSync(file, "utf8").replace("new Box(1)", "new Box(2)"));
    expect(runTs(file).stdout).toBe("red 2\n");
  });

  it("follows Node's format rules: package type, else syntax, enums included", () => {
    // No `package.json` above the scratch directory: detected by syntax.
    const esm = join(dir, "esm.ts");
    writeFileSync(
      esm,
      `import { basename } from "node:path";
enum Kind { Module = "module" }
console.log(Kind.Module, basename(import.meta.filename));
`,
    );
    const detected = runTs(esm);
    expect(detected.status, detected.stderr).toBe(0);
    expect(detected.stdout).toBe("module esm.ts\n");

    // A `type: "commonjs"` package makes the same syntax an error, as in Node.
    const typed = join(dir, "typed");
    mkdirSync(typed);
    writeFileSync(join(typed, "package.json"), JSON.stringify({ type: "commonjs" }));
    writeFileSync(
      join(typed, "cjs.ts"),
      `enum Kind { Cjs = "cjs" }\nconsole.log(Kind.Cjs, typeof module);\n`,
    );
    expect(runTs(join(typed, "cjs.ts")).stdout).toBe("cjs object\n");
    writeFileSync(join(typed, "esm.ts"), readFileSync(esm, "utf8"));
    expect(runTs(join(typed, "esm.ts")).status).not.toBe(0);
  });
});

describe("cachedQuickjsBundle", () => {
  it("rebuilds when a transform helper or the cached bundle changes", async () => {
    const bundles = join(dir, "quickjs");
    const first = await cachedQuickjsBundle(workflow, bundles);
    expect(first.cached).toBe(false);
    expect((await cachedQuickjsBundle(workflow, bundles)).cached).toBe(true);

    // The helper is bundled only into the transform artifact.
    writeFileSync(
      helper,
      `export const rewrite = (text: string): string => text.replace("a", "two");\n`,
    );
    const second = await cachedQuickjsBundle(workflow, bundles);
    expect(second.cached).toBe(false);
    expect(second.hash).not.toBe(first.hash);
    expect(readFileSync(second.path, "utf8")).toContain("two");

    // A bundle that no longer matches its recorded hash is rebuilt.
    writeFileSync(second.path, "export function main() {}\n");
    const third = await cachedQuickjsBundle(workflow, bundles);
    expect(third).toEqual({ ...second, cached: false });
    expect(readFileSync(third.path, "utf8")).toContain("two");
  });
});

/**
 * Startup caches (`src/bundle/cache.ts`): the type-stripping cache of
 * `bin/register-ts.mjs` and the quickjs bundle cache. Every change to a
 * module, a bundled helper, or a cache entry must rebuild.
 *
 * The module hooks are exercised in child processes: Node caches modules per
 * URL for the life of a process, and vitest's own loader would bypass them.
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
import { afterAll, beforeEach, describe, expect, it } from "vitest";
import { privateDirectory } from "../src/bundle/cache.ts";
import { cachedQuickjsBundle } from "../src/host/quickjs-bundle.ts";

const register = resolve(import.meta.dirname, "../bin/register-ts.mjs");
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

describe("the cache directory", () => {
  it("is not used when other users can write to it", () => {
    mkdirSync(cache, { recursive: true });
    chmodSync(cache, 0o777);
    try {
      expect(privateDirectory("quickjs", cache)).toBeUndefined();
      expect(readdirSync(cache)).toEqual([]);
    } finally {
      chmodSync(cache, 0o700);
    }
    expect(privateDirectory("quickjs", cache)).toBe(join(cache, "quickjs"));
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

/**
 * The quickjs bundle (`src/host/quickjs-bundle.ts`, `codemod-workflow
 * build --runtime=quickjs`): self-contained, Node-free, async lowered, and
 * carrying exactly the artifacts (same hashes) the bridge runtime would run.
 */
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { buildFile } from "../src/index.ts";
import { runWorkflowCli } from "../src/host/cli.ts";
import { buildQuickjsBundle, cachedQuickjsBundle } from "../src/host/quickjs-bundle.ts";

const fixtures = resolve(import.meta.dirname, "fixtures");
const scratch = mkdtempSync(join(tmpdir(), "quickjs-bundle-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

describe("buildQuickjsBundle", () => {
  it("is one ES module with no imports, lowered async, and the bridge's artifacts", async () => {
    const bundle = await buildQuickjsBundle(join(fixtures, "quickjs/jssg-sequence.ts"));
    expect(bundle.hash).toBe(createHash("sha256").update(bundle.code).digest("hex"));
    expect(bundle.code).not.toMatch(/^\s*import\s/mu);
    expect(bundle.code).not.toMatch(/\bnode:/u);
    // Line 1 is the artifact table (transform sources stay as shipped).
    const body = bundle.code
      .slice(bundle.code.indexOf("\n") + 1)
      .replace(/`(?:[^`\\]|\\.)*`|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\/\/.*$/gmu, '""');
    expect(body).toContain("__async(");
    expect(body).not.toMatch(/\basync\b|\bawait\b/u);
    expect(bundle.code).toMatch(/export\s*\{[^}]*\bmain\b/u);

    const expected = buildFile(join(fixtures, "jssg/transforms.ts")).artifacts;
    expect(
      bundle.artifacts
        .map(({ name, hash }) => ({ name, hash }))
        .sort((a, b) => (a.name < b.name ? -1 : 1)),
    ).toEqual(
      expected.map(({ name, hash }) => ({ name, hash })).sort((a, b) => (a.name < b.name ? -1 : 1)),
    );
    for (const artifact of expected) expect(bundle.code).toContain(artifact.hash);
    expect(bundle.inputs).toContain(join(fixtures, "jssg/helpers.ts"));
  });

  it("rejects node:* imports in the workflow graph", async () => {
    const file = join(scratch, "node-import.ts");
    writeFileSync(
      file,
      `import { readFileSync } from "node:fs";\nimport { shell } from ${JSON.stringify(resolve(import.meta.dirname, "../src/index.ts"))};\nexport default shell({ name: "x", command: readFileSync("x", "utf8") });\n`,
    );
    await expect(buildQuickjsBundle(file)).rejects.toThrow(/'node:fs'.*Node module/su);
  });

  it("caches by input content and rebuilds when an input changes", async () => {
    const dir = join(scratch, "cache-case");
    const workflow = join(dir, "workflow.ts");
    const lib = join(dir, "lib.ts");
    const { mkdirSync } = await import("node:fs");
    mkdirSync(dir, { recursive: true });
    writeFileSync(lib, `export const command = "echo one";\n`);
    writeFileSync(
      workflow,
      `import { shell } from ${JSON.stringify(resolve(import.meta.dirname, "../src/index.ts"))};\nimport { command } from "./lib.ts";\nexport default shell({ name: "x", command });\n`,
    );
    const cache = join(scratch, "cache");
    const first = await cachedQuickjsBundle(workflow, cache);
    const second = await cachedQuickjsBundle(workflow, cache);
    expect(first.cached).toBe(false);
    expect(second).toEqual({ ...first, cached: true });
    writeFileSync(lib, `export const command = "echo two";\n`);
    const third = await cachedQuickjsBundle(workflow, cache);
    expect(third.cached).toBe(false);
    expect(third.hash).not.toBe(first.hash);
    expect(readFileSync(third.path, "utf8")).toContain("echo two");
  });
});

describe("codemod-workflow build", () => {
  it("writes the bundle and summarizes it", async () => {
    const out = join(scratch, "out/bundle.mjs");
    const summary = (await runWorkflowCli([
      "build",
      join(fixtures, "quickjs/jssg-sequence.ts"),
      "--runtime=quickjs",
      "-o",
      out,
    ])) as { bundle: string; hash: string; artifacts: { name: string }[] };
    expect(summary.bundle).toBe(out);
    expect(summary.hash).toBe(createHash("sha256").update(readFileSync(out)).digest("hex"));
    expect(summary.artifacts.map((a) => a.name)).toContain("replace");
  });

  it("builds bundles for the quickjs runtime only", async () => {
    await expect(
      runWorkflowCli([
        "build",
        join(fixtures, "quickjs/jssg-sequence.ts"),
        "--runtime=bridge",
        "-o",
        "x",
      ]),
    ).rejects.toThrow(/quickjs only/u);
  });
});

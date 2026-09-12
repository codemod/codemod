/**
 * The build step: inline transforms become standalone artifacts (helpers
 * bundled, identity by content hash, no paths), the workflow module is
 * rewritten to reference them, unsupported capture is refused with a
 * position, and the trusted-local loader applies it to a whole module graph.
 */
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { BuildError, buildFile, buildModule } from "../src/index.ts";
import { sha256 } from "./helpers.ts";

const HEADER = `import { jssg, workflow } from "@codemod.com/orchestration";
import type { SgRoot } from "@codemod.com/jssg-types/main";
import { migrateText, OLD_API } from "./helpers.ts";
import * as helpers from "./helpers.ts";
import transformFn from "./helpers.ts";
`;

const METHOD = `${HEADER}
const migrate = jssg({
  name: "migrate",
  language: "typescript",
  transform(root: SgRoot, options) {
    const input = options.params.input;
    return { content: migrateText(root.root().text()), output: { input } };
  },
});
export default workflow(() => migrate());
`;

let dir: string;
const build = (source: string, file = "workflow.ts") => buildModule(source, join(dir, file));

beforeAll(() => {
  dir = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-build-")));
  mkdirSync(join(dir, "sub"));
  writeFileSync(
    join(dir, "sub", "format.ts"),
    "export const banner = (s: string) => `// ${s}\\n`;\n",
  );
  writeFileSync(
    join(dir, "helpers.ts"),
    `import { banner } from "./sub/format.ts";
export const OLD_API = "oldApi";
export function migrateText(text: string): string { return banner("migrated") + text.replaceAll(OLD_API, "newApi"); }
export function unused(): number { return 1; }
export default function transformFn(root: { root(): { text(): string } }) { return migrateText(root.root().text()); }
`,
  );
});
afterAll(() => rmSync(dir, { recursive: true, force: true }));

describe("buildModule", () => {
  it("extracts a method transform into a self-contained artifact and rewrites the call", () => {
    const { source, artifacts } = build(METHOD);

    expect(artifacts).toHaveLength(1);
    const [artifact] = artifacts;
    expect(artifact!.name).toBe("migrate");
    expect(artifact!.hash).toBe(sha256(artifact!.source));
    expect(artifact!.origin).toBe(join(dir, "workflow.ts"));
    // The transform and the helpers it reaches, transitively, nothing else.
    expect(artifact!.source).toContain("function migrateText");
    expect(artifact!.source).toContain("var banner");
    expect(artifact!.source).toContain("options.params.input");
    expect(artifact!.source).toMatch(/export \{\s+transform as default\s+\}/u);
    expect(artifact!.source).not.toContain("unused");
    expect(artifact!.source).not.toContain("workflow(");
    expect(artifact!.source).not.toContain("orchestration");
    expect(artifact!.source).not.toContain(dir);
    // The module keeps everything else and now references the artifact.
    expect(source).toContain(
      `transform: ${JSON.stringify({ name: "migrate", hash: artifact!.hash })}`,
    );
    expect(source).not.toContain("transform(root");
    expect(source).toContain("export default workflow(() => migrate());");
  });

  it.each<[string, string, RegExp]>([
    // (form, transform property, what the artifact must contain)
    ["an arrow function", "transform: (root) => migrateText(root.root().text())", /migrateText\(/u],
    [
      "an async function expression",
      "transform: async function (root) { return migrateText(root.root().text()); }",
      /async function/u,
    ],
    ["an imported binding", "transform: transformFn", /function transformFn/u],
    [
      "a namespace member is a capture-free reference",
      "transform: (root) => helpers.migrateText(root.root().text())",
      /migrateText/u,
    ],
    ["a shorthand imported binding", "transform", /var workflow_default = transformFn;/u],
  ])("accepts %s", (_form, property, expected) => {
    const withTransform = `${HEADER}import transform from "./helpers.ts";
export const t = jssg({ name: "t", language: "tsx", ${property} });
`;
    const { source, artifacts } = build(withTransform);
    expect(artifacts[0]!.source).toMatch(expected);
    expect(source).toMatch(/transform: \{"name":"t","hash":"[0-9a-f]{64}"\}/u);
  });

  it.each<[string, string, RegExp]>([
    // (what is wrong, module body after the header, expected message)
    [
      "a captured top-level const",
      `const PATTERN = "x";\nexport const t = jssg({ name: "t", language: "tsx", transform(root) { return root.root().text().replace(PATTERN, ""); } });`,
      /workflow\.ts:8:\d+: jssg 't' transform uses 'PATTERN', declared in the workflow module at line 7; .*import it, or pass it through invocation input/u,
    ],
    [
      "a captured top-level function",
      `function helper() { return 1; }\nexport const t = jssg({ name: "t", language: "tsx", transform() { return String(helper()); } });`,
      /uses 'helper', declared in the workflow module at line 7/u,
    ],
    [
      "a captured top-level class",
      `class Box {}\nexport const t = jssg({ name: "t", language: "tsx", transform() { return String(new Box()); } });`,
      /uses 'Box', declared in the workflow module/u,
    ],
    [
      "a capture in a nested arrow",
      `let count = 0;\nexport const t = jssg({ name: "t", language: "tsx", transform(root) { return [1].map(() => count++).join(); } });`,
      /uses 'count'/u,
    ],
    [
      "the orchestration runtime inside the transform",
      `export const t = jssg({ name: "t", language: "tsx", transform() { return String(workflow); } });`,
      /uses 'workflow' from @codemod\.com\/orchestration; the orchestration runtime is not available inside a transform/u,
    ],
    [
      "a computed name",
      `const NAME = "t";\nexport const t = jssg({ name: NAME, language: "tsx", transform() { return null; } });`,
      /jssg name must be a string literal/u,
    ],
    [
      "a missing transform",
      `export const t = jssg({ name: "t", language: "tsx" });`,
      /jssg 't' has no transform/u,
    ],
    [
      "a generator transform",
      `export const t = jssg({ name: "t", language: "tsx", *transform() {} });`,
      /cannot be a generator/u,
    ],
    [
      "a transform that is not a function",
      `export const t = jssg({ name: "t", language: "tsx", transform: 42 });`,
      /must be a method, a function or arrow expression, or an imported binding/u,
    ],
    [
      "a spread argument",
      `const options = { name: "t" };\nexport const t = jssg(options);`,
      /jssg\(\) must be called with an object literal/u,
    ],
    [
      "an import that does not resolve",
      `import { nope } from "./missing.ts";\nexport const t = jssg({ name: "t", language: "tsx", transform() { return nope(); } });`,
      /failed to bundle a jssg transform: .*missing\.ts/u,
    ],
  ])("rejects %s with a position", (_what, body, message) => {
    const source = `${HEADER}\n${body}\n`;
    expect(() => build(source)).toThrow(BuildError);
    expect(() => build(source)).toThrow(message);
    expect(() => build(source)).toThrow(/^.*workflow\.ts:\d+:\d+: |^.*workflow\.ts: /u);
  });

  it("treats names bound inside the transform, property names, and globals as non-captures", () => {
    const body = `
const PATTERN = "module";
const config = { PATTERN: 1 };
function helper() {}
class Box {}
export const t = jssg({
  name: "t",
  language: "tsx",
  transform(root, { params: { input } }) {
    const PATTERN = "local";
    function helper() { return PATTERN; }
    for (const Box of [1]) console.log(Box);
    try { throw 1; } catch (config) { console.log(config); }
    const named = class helper {};
    label: for (const [helper2] of [[1]]) { if (helper2) break label; }
    const { PATTERN: renamed = undefined } = { PATTERN: input };
    return JSON.stringify({ helper: helper(), config: { PATTERN }.PATTERN, named: typeof named, renamed, input });
  },
});`;
    const { artifacts } = build(`${HEADER}${body}`);
    expect(artifacts[0]!.source).toContain('"local"');
    expect(artifacts[0]!.source).not.toContain('"module"');
  });

  it("gives every definition in a module its own artifact", () => {
    const body = `
export const a = jssg({ name: "a", language: "tsx", transform: () => "a" });
export const b = jssg({ name: "b", language: "typescript", transform: () => "b" });`;
    const { source, artifacts } = build(`${HEADER}${body}`);
    expect(artifacts.map((artifact) => artifact.name)).toEqual(["a", "b"]);
    expect(source.match(/transform: \{"name":"[ab]","hash"/gu)).toHaveLength(2);
  });

  it("hashes the bundled content: stable across checkouts, sensitive to helpers", () => {
    const first = build(METHOD).artifacts[0]!;
    const moved = mkdtempSync(join(tmpdir(), "codemod-moved-"));
    try {
      cpSync(dir, join(moved, "checkout"), { recursive: true });
      const second = buildModule(METHOD, join(moved, "checkout", "workflow.ts")).artifacts[0]!;
      expect(second.hash).toBe(first.hash);
      expect(second.source).toBe(first.source);

      writeFileSync(
        join(moved, "checkout", "sub", "format.ts"),
        "export const banner = () => '';\n",
      );
      const helperChanged = buildModule(METHOD, join(moved, "checkout", "workflow.ts"));
      expect(helperChanged.artifacts[0]!.hash).not.toBe(first.hash);
    } finally {
      rmSync(moved, { recursive: true, force: true });
    }
    const bodyChanged = build(METHOD.replace("output: { input }", "output: { input, v: 2 }"));
    expect(bodyChanged.artifacts[0]!.hash).not.toBe(first.hash);
  });
});

describe("loadWorkflow through codemod-workflow", () => {
  it("splits every module in the graph and hands the artifacts to the executor", () => {
    // The fake bridge echoes the artifact source it received, so a run
    // without Rust still shows the loader's artifacts reaching the executor.
    const repo = mkdtempSync(join(tmpdir(), "codemod-load-"));
    writeFileSync(join(repo, "a.ts"), "a\n");
    const workflowPath = resolve(import.meta.dirname, "fixtures/load/workflow.ts");
    try {
      const result = spawnSync(
        process.execPath,
        [
          resolve(import.meta.dirname, "../bin/codemod-workflow.mjs"),
          workflowPath,
          "--target",
          repo,
          "--bridge",
          resolve(import.meta.dirname, "fixtures/fake-bridge.mjs"),
        ],
        { encoding: "utf8" },
      );
      expect(result.status, result.stderr).toBe(0);
      const built = buildFile(join(dirname(workflowPath), "definitions.ts")).artifacts[0]!;
      const output = JSON.parse(result.stdout) as {
        transform: unknown;
        output: { path: string; context: { artifact: { source: string } } }[];
      };
      expect(output.transform).toEqual({ name: "migrate", hash: built.hash });
      expect(output.output[0]!.path).toBe("a.ts");
      expect(output.output[0]!.context.artifact.source).toBe(built.source);
    } finally {
      rmSync(repo, { recursive: true, force: true });
    }
  });
});

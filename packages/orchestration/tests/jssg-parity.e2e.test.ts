/**
 * Cross-runtime JSSG parity: the same edge-case commands
 * (`fixtures/parity/`) run through `codemod-workflow run` on the `node`
 * and `quickjs` runtimes against identical repositories. Both reach the same
 * Rust batch rules (`crates/jssg-batch`: planning, path containment, the
 * selector skip, result conversion), so outputs, file trees, and errors
 * must be identical: errors produced by those rules byte-for-byte; errors a
 * transform throws byte-for-byte once stack frames are removed. The YAML
 * engine is the reference for the benchmark transforms
 * (`benchmarks/runtimes/verify.sh`).
 *
 * Run with: pnpm --filter @codemod.com/orchestration test:e2e
 * (needs the napi addon and the QuickJS host; the script builds both).
 * Override the host binary with CODEMOD_QUICKJS_HOST_BIN.
 */
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { isAvailable as isNapiAvailable } from "@codemod.com/jssg-napi";

const enabled = process.env.CODEMOD_QUICKJS_E2E === "1";
const repoRoot = resolve(import.meta.dirname, "../../..");
const host =
  process.env.CODEMOD_QUICKJS_HOST_BIN ?? join(repoRoot, "target/debug/orchestration-quickjs-host");
const cli = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");
const workflow = resolve(import.meta.dirname, "fixtures/parity/workflow.ts");
const scratch = realpathSync(mkdtempSync(join(tmpdir(), "jssg-parity-")));

const RUNTIMES = ["node", "quickjs"] as const;
type Runtime = (typeof RUNTIMES)[number];

beforeAll(() => {
  if (!enabled) return;
  if (!existsSync(host)) throw new Error(`quickjs host binary not found at ${host}`);
  if (!isNapiAvailable()) {
    throw new Error(
      "jssg napi addon not built; run 'pnpm --filter @codemod.com/jssg-napi build:debug'",
    );
  }
});
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

let repos = 0;

interface Setup {
  files: Record<string, string>;
  /** Directory symlinks inside the repository pointing at a fresh directory outside it. */
  outsideLinks?: string[];
  /** Transform pool threads (`CODEMOD_JSSG_WORKERS`); default 1. */
  workers?: number;
}

interface Outcome {
  ok: boolean;
  /** The repository before the run (symlinked directories included). */
  initial: Record<string, string>;
  output?: unknown;
  error?: string;
  /** The files a successful run skipped, as it reported them. */
  skipped?: string[];
  tree: Record<string, string>;
  outside: Record<string, string>;
}

function tree(root: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const name of readdirSync(root, { recursive: true, encoding: "utf8" }).sort()) {
    try {
      out[name.split("\\").join("/")] = readFileSync(join(root, name), "utf8");
    } catch {
      // a directory (or a symlink to one)
    }
  }
  return out;
}

/** The error text with paths of this run and stack frames removed. */
function normalize(text: string, repo: string): string {
  return text
    .split(repo)
    .join("<REPO>")
    .split("\n")
    .filter((line) => !/^\s+at /u.test(line))
    .join("\n")
    .trim();
}

function runOn(runtime: Runtime, setup: Setup, input: unknown): Outcome {
  const dir = join(scratch, `case-${++repos}`);
  const repo = join(dir, "repo");
  const outside = join(dir, "outside");
  mkdirSync(outside, { recursive: true });
  writeFileSync(join(outside, "keep.ts"), "keep;\n");
  for (const [path, content] of Object.entries(setup.files)) {
    mkdirSync(dirname(join(repo, path)), { recursive: true });
    writeFileSync(join(repo, path), content);
  }
  for (const link of setup.outsideLinks ?? []) symlinkSync(outside, join(repo, link));
  const initial = tree(repo);
  const result = spawnSync(
    process.execPath,
    [
      cli,
      "run",
      `--runtime=${runtime}`,
      workflow,
      "--target",
      repo,
      "--input",
      JSON.stringify(input),
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        CODEMOD_QUICKJS_HOST_BIN: host,
        CODEMOD_WORKFLOW_CACHE: join(scratch, "cache"),
        CODEMOD_JSSG_WORKERS: String(setup.workers ?? 1),
      },
    },
  );
  const outcome: Outcome = {
    ok: result.status === 0,
    initial,
    tree: tree(repo),
    outside: tree(outside),
  };
  if (outcome.ok) {
    outcome.output = JSON.parse(result.stdout) as unknown;
    outcome.skipped = normalize(result.stderr, repo)
      .split("\n")
      .filter((line) => line.includes("skipped"));
  } else outcome.error = normalize(result.stderr, repo);
  return outcome;
}

/** Runs one case on every runtime; asserts identical results and returns them. */
function parity(
  setup: Setup,
  input: unknown,
  expected: { ok: true } | { error: string; thrown?: true },
): Record<Runtime, Outcome> {
  const outcomes = Object.fromEntries(
    RUNTIMES.map((runtime) => [runtime, runOn(runtime, setup, input)]),
  ) as Record<Runtime, Outcome>;
  const context = JSON.stringify(outcomes, null, 2);
  for (const runtime of RUNTIMES) {
    const outcome = outcomes[runtime];
    expect(outcome.outside, `${runtime}: outside the root\n${context}`).toEqual({
      "keep.ts": "keep;\n",
    });
    if ("ok" in expected) {
      expect(outcome.ok, `${runtime} failed\n${context}`).toBe(true);
    } else {
      expect(outcome.ok, `${runtime} succeeded\n${context}`).toBe(false);
      expect(outcome.error, `${runtime}\n${context}`).toContain(expected.error);
      // Failing commands write nothing.
      expect(outcome.tree, `${runtime}: tree\n${context}`).toEqual(outcome.initial);
    }
  }
  expect(outcomes.quickjs, `quickjs vs node\n${context}`).toEqual(outcomes.node);
  return outcomes;
}

const TWO = { "a.ts": "oldApi(1);\n", "b.ts": "plain(2);\n" };

describe.skipIf(!enabled)("JSSG parity: node, quickjs", () => {
  it("string results: changed, unchanged; nullish results", () => {
    const changed = parity({ files: TWO }, { step: "returns", input: "changed" }, { ok: true });
    expect(changed.node.tree["a.ts"]).toBe("newApi(1);\n");
    for (const input of ["unchanged", "null", "undefined", "structured-null"]) {
      const same = parity({ files: TWO }, { step: "returns", input }, { ok: true });
      expect(same.quickjs.tree).toEqual(TWO);
    }
  });

  it("invalid results fail with the sandbox's error", () => {
    for (const [input, type] of [
      ["int", "int"],
      ["float", "float"],
      ["bool", "bool"],
      ["array", "object"],
      ["structured-bad-content", "int"],
    ]) {
      const message =
        input === "array"
          ? "Object codemod results must contain an 'output' field"
          : `must return either a string or null/undefined. Received ${type}`;
      parity({ files: TWO }, { step: "returns", input }, { error: message });
    }
    parity(
      { files: TWO },
      { step: "returns", input: "missing-output" },
      { error: "Object codemod results must contain an 'output' field" },
    );
  });

  it("structured outputs convert to the same JSON", () => {
    const shapes = parity(
      { files: TWO },
      { step: "returns", input: "output-shapes" },
      { ok: true },
    );
    expect(shapes.node.output).toEqual([
      {
        file: "a.ts",
        nested: { list: [1, "two", null, null, 2.5, -3, [true]] },
        missing: null,
        fn: null,
        big: 2 ** 40,
        empty: {},
      },
      {
        file: "b.ts",
        nested: { list: [1, "two", null, null, 2.5, -3, [true]] },
        missing: null,
        fn: null,
        big: 2 ** 40,
        empty: {},
      },
    ]);
  });

  it("a thrown error fails the command and writes nothing", () => {
    parity({ files: TWO }, { step: "throwing" }, { error: "JSSG failed for 'b.ts'", thrown: true });
  });

  it("a file whose transform throws is skipped and reported; the others are written", () => {
    const outcomes = parity({ files: TWO }, { step: "skipping" }, { ok: true });
    expect(outcomes.node.tree).toEqual({ "a.ts": "newApi(1);\n", "b.ts": TWO["b.ts"] });
    expect(outcomes.node.skipped).toEqual([
      "jssg 'skipping': skipped JSSG failed for 'b.ts': Error: b.ts is not allowed",
    ]);
  });

  it("a batch on the transform pool writes, outputs, and skips as one thread does", () => {
    const files = Object.fromEntries(
      Array.from({ length: 80 }, (_, n) => [
        `f${String(n).padStart(2, "0")}${n % 9 === 0 ? "b" : ""}.ts`,
        n % 3 === 0 ? `plain(${n});\n` : `oldApi(${n});\n`,
      ]),
    );
    for (const step of ["selected", "skipping"]) {
      const serial = parity({ files }, { step }, { ok: true });
      const pooled = parity({ files, workers: 3 }, { step }, { ok: true });
      expect(pooled.node).toEqual(serial.node);
    }
  });

  it("the static selector skips files before the transform runs", () => {
    const files = { "a.ts": "oldApi(1);\n", "b.ts": "plain(2);\n", "c/d.ts": "x(oldApi(3));\n" };
    const outcomes = parity({ files }, { step: "selected" }, { ok: true });
    expect(outcomes.node.output).toEqual(["a.ts", "c/d.ts"]);
  });

  it("multiple edits, a staged write(), and a staged jssgTransform()", () => {
    const files = {
      "main.ts": "oldApi(1);\noldApi(2);\nkeep();\n",
      "lib/other.ts": "oldApi(3);\n",
      "lib/plain.ts": "plain();\n",
    };
    const outcomes = parity({ files }, { step: "multi", include: ["main.ts"] }, { ok: true });
    expect(outcomes.quickjs.tree).toEqual({
      "lib/other.ts": "newApi(3);\n",
      "lib/plain.ts": "export const written = true;\n",
      "main.ts": "newApi(1);\nnewApi(2);\nkeep();\n",
    });
    expect(outcomes.quickjs.output).toEqual([2]);
  });

  it("renames inside the root; renames and secondary writes outside it are rejected", () => {
    const renamed = parity(
      { files: TWO },
      { step: "renameInto", input: "moved", include: ["a.ts"] },
      { ok: true },
    );
    expect(renamed.node.tree).toEqual({ "b.ts": TWO["b.ts"], "moved/a.ts": TWO["a.ts"] });
    // `..` is refused by `rename()` itself.
    parity(
      { files: TWO },
      { step: "renameInto", input: "..", include: ["a.ts"] },
      { error: "outside the target directory", thrown: true },
    );
    // Through a symlinked directory: `rename()` resolves it and refuses too
    // (converting the outcome would refuse it again: `crates/jssg-batch` tests).
    parity(
      { files: TWO, outsideLinks: ["out"] },
      { step: "renameInto", input: "out", include: ["a.ts"] },
      { error: "is outside the target directory", thrown: true },
    );
    parity(
      { files: TWO, outsideLinks: ["out"] },
      { step: "secondaryAt", input: "out/keep.ts", include: ["a.ts"] },
      { error: "outside the target directory", thrown: true },
    );
    parity(
      { files: TWO },
      { step: "secondaryAt", input: "../escaped.ts", include: ["a.ts"] },
      { error: "outside the target directory", thrown: true },
    );
  });

  it("BOM, CRLF, and no trailing newline: the same input and bytes as the workflow engine", () => {
    const outcomes = parity({ files: ENCODING_FILES }, { step: "encoding" }, { ok: true });
    // `parity` already required quickjs to equal node: the same
    // trees (read back losslessly, a BOM included) and the same outputs.
    const { tree } = outcomes.node;
    const seen = (path: string) =>
      JSON.parse(/\/\* seen (.*) \*\/$/su.exec(tree[path]!)![1]!) as {
        head: number[];
        length: number;
        crlf: boolean;
        root: { start: { index: number; column: number } };
      };
    // The transform sees the file verbatim: tree-sitter starts the root
    // after a leading BOM, so `root().text()` lacks it but offsets count it.
    expect(seen("bom-edit.ts")).toMatchObject({ head: [111, 108], length: 11, crlf: false });
    expect(seen("bom-edit.ts").root.start).toMatchObject({ index: 3, column: 1 });
    expect(seen("edit.ts").root.start).toMatchObject({ index: 0, column: 0 });
    expect(seen("bom-crlf-edit.ts")).toMatchObject({ crlf: true, root: { start: { index: 3 } } });
    expect(seen("double-bom-edit.ts").root.start).toMatchObject({ index: 6 });
    // Written bytes: line endings and a missing final newline are kept; an
    // edited BOM file is the transform's text (no BOM, as the engine writes
    // it); `null` leaves a file alone; `rename()` keeps the BOM; returning
    // `root().text()` of a BOM file is a change (it differs from the file).
    expect(tree["crlf-edit.ts"]).toMatch(/^newApi\(1\);\r\nnewApi\(2\);\r\n\/\* seen /u);
    expect(tree["notrail-edit.ts"]).toMatch(/^newApi\(1\);\/\* seen /u);
    expect(tree["bom-edit.ts"]).toMatch(/^newApi\(1\);\n\/\* seen /u);
    expect(tree["bom-null.ts"]).toBe(`${BOM}keep();\n`);
    expect(tree["bom-renamed.ts"]).toBe(`${BOM}keep();\n`);
    expect(tree["crlf-renamed.ts"]).toBe("keep();\r\n");
    expect(tree["bom-rename.ts"]).toBeUndefined();
    expect(tree["bom-same.ts"]).toBe("keep();\n");
    expect(tree["crlf-same.ts"]).toBe("keep();\r\n");

    // The workflow engine (`codemod workflow run`, a `js-ast-grep` step over
    // the same transform file), when a `codemod` binary is available.
    if (codemodCli === undefined) {
      if (process.env.CODEMOD_REQUIRE_RUNTIMES === "1") {
        throw new Error("codemod CLI not found; build it or set CODEMOD_BIN");
      }
      return;
    }
    expect(runEngine(ENCODING_FILES)).toEqual(tree);
  });
});

const BOM = "﻿";

/** Files for the `encoding` step, named for what `fixtures/parity/encoding.ts` does with them. */
const ENCODING_FILES: Record<string, string> = {
  "edit.ts": "oldApi(1);\n",
  "bom-edit.ts": `${BOM}oldApi(1);\n`,
  "bom-crlf-edit.ts": `${BOM}oldApi(1);\r\noldApi(2);\r\n`,
  "crlf-edit.ts": "oldApi(1);\r\noldApi(2);\r\n",
  "notrail-edit.ts": "oldApi(1);",
  "bom-notrail-edit.ts": `${BOM}oldApi(1);`,
  "double-bom-edit.ts": `${BOM}${BOM}oldApi(1);\n`,
  "mid-bom-edit.ts": `oldApi(1);\n${BOM}x;\n`,
  "bom-same.ts": `${BOM}keep();\n`,
  "crlf-same.ts": "keep();\r\n",
  "bom-rename.ts": `${BOM}keep();\n`,
  "crlf-rename.ts": "keep();\r\n",
  "bom-null.ts": `${BOM}keep();\n`,
};

const codemodCli = [
  process.env.CODEMOD_BIN,
  join(repoRoot, "target/release/codemod"),
  join(repoRoot, "target/debug/codemod"),
].find((path) => path !== undefined && existsSync(path));

/** Runs `fixtures/parity/encoding.ts` as a workflow engine step; returns the tree. */
function runEngine(files: Record<string, string>): Record<string, string> {
  const dir = join(scratch, `engine-${++repos}`);
  const repo = join(dir, "repo");
  const definition = join(dir, "workflow");
  mkdirSync(definition, { recursive: true });
  copyFileSync(
    resolve(import.meta.dirname, "fixtures/parity/encoding.ts"),
    join(definition, "encoding.ts"),
  );
  writeFileSync(
    join(definition, "workflow.yaml"),
    [
      'version: "1"',
      "nodes:",
      "  - id: encoding",
      "    name: encoding",
      "    type: automatic",
      "    steps:",
      "      - name: encoding",
      "        js-ast-grep:",
      "          js_file: encoding.ts",
      "          language: typescript",
      '          include: ["**/*.ts"]',
      "",
    ].join("\n"),
  );
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(repo, path)), { recursive: true });
    writeFileSync(join(repo, path), content);
  }
  const result = spawnSync(
    codemodCli!,
    [
      "workflow",
      "run",
      "-w",
      join(definition, "workflow.yaml"),
      "--target",
      repo,
      "--no-interactive",
      "--disable-analytics",
      "--allow-dirty",
    ],
    { encoding: "utf8", env: { ...process.env, DISABLE_ANALYTICS: "true" } },
  );
  expect(result.status, `workflow engine failed\n${result.stdout}\n${result.stderr}`).toBe(0);
  return tree(repo);
}

/**
 * Cross-runtime JSSG parity: the same edge-case commands
 * (`fixtures/parity/`) run through `codemod-workflow run` on the `bridge`,
 * `node`, and `quickjs` runtimes against identical repositories. All three
 * reach the same Rust batch rules (`crates/jssg-batch`: planning, path
 * containment, the selector skip, result conversion), so outputs, file
 * trees, and errors must be identical: errors produced by those rules
 * byte-for-byte; errors a transform throws byte-for-byte once the bridge
 * sandbox's wrapper and stack frames are removed.
 *
 * Run with: pnpm --filter @codemod.com/orchestration test:e2e
 * (needs the bridge, the napi addon, and the QuickJS host; the script builds
 * all three). Override the binaries with CODEMOD_BRIDGE_BIN and
 * CODEMOD_QUICKJS_HOST_BIN.
 */
import { spawnSync } from "node:child_process";
import {
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

const enabled = process.env.CODEMOD_QUICKJS_E2E === "1" || process.env.CODEMOD_BRIDGE_E2E === "1";
const repoRoot = resolve(import.meta.dirname, "../../..");
const bridge =
  process.env.CODEMOD_BRIDGE_BIN ?? join(repoRoot, "target/debug/butterflow-execution-bridge");
const host =
  process.env.CODEMOD_QUICKJS_HOST_BIN ?? join(repoRoot, "target/debug/orchestration-quickjs-host");
const cli = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");
const workflow = resolve(import.meta.dirname, "fixtures/parity/workflow.ts");
const scratch = realpathSync(mkdtempSync(join(tmpdir(), "jssg-parity-")));

const RUNTIMES = ["bridge", "node", "quickjs"] as const;
type Runtime = (typeof RUNTIMES)[number];

beforeAll(() => {
  if (!enabled) return;
  for (const [name, path] of [
    ["bridge", bridge],
    ["quickjs host", host],
  ] as const) {
    if (!existsSync(path)) throw new Error(`${name} binary not found at ${path}`);
  }
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
}

interface Outcome {
  ok: boolean;
  /** The repository before the run (symlinked directories included). */
  initial: Record<string, string>;
  output?: unknown;
  error?: string;
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
      "--bridge",
      bridge,
      "--input",
      JSON.stringify(input),
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        CODEMOD_QUICKJS_HOST_BIN: host,
        CODEMOD_WORKFLOW_CACHE: join(scratch, "cache"),
      },
    },
  );
  const outcome: Outcome = {
    ok: result.status === 0,
    initial,
    tree: tree(repo),
    outside: tree(outside),
  };
  if (outcome.ok) outcome.output = JSON.parse(result.stdout) as unknown;
  else outcome.error = normalize(result.stderr, repo);
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
  for (const runtime of ["node", "quickjs"] as const) {
    const { error, ...rest } = outcomes[runtime];
    const { error: reference, ...bridgeRest } = outcomes.bridge;
    expect(rest, `${runtime} vs bridge\n${context}`).toEqual(bridgeRest);
    // An error the transform throws reaches the bridge through its sandbox,
    // which wraps it; past that wrapper (and the stack) it is the same text.
    const unwrap = (text: string | undefined) =>
      "thrown" in expected ? text?.replace(SANDBOX_WRAPPER, "") : text;
    expect(unwrap(error), `${runtime} vs bridge\n${context}`).toBe(unwrap(reference));
  }
  return outcomes;
}

/** How the bridge's per-file sandbox wraps an error a transform throws. */
const SANDBOX_WRAPPER = "Runtime error JavaScript execution failed: ";

const TWO = { "a.ts": "oldApi(1);\n", "b.ts": "plain(2);\n" };

describe.skipIf(!enabled)("JSSG parity: bridge, node, quickjs", () => {
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
    expect(shapes.bridge.output).toEqual([
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
});

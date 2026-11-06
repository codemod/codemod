/**
 * Parity of the `node` runtime with the `bridge` runtime: the same JSSG
 * fixtures run through `executeJssgCommand` with the real bridge process and
 * with the in-process napi dispatch, on identical repositories. The batch
 * results (`FileOutcome`s), the command completion, and the resulting file
 * trees must be identical; failures must fail in the same phase with the
 * repository untouched. Whole workflows (shell + jssg) are compared through
 * `runWorkflow` as well.
 *
 * Run with: pnpm --filter @codemod.com/orchestration test:e2e
 * (needs the bridge binary and the napi addon; both are built by the script).
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
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { isAvailable as isNapiAvailable } from "@codemod.com/jssg-napi";
import {
  MemoryHistoryStore,
  NODE_RUNTIME,
  NodeExecutor,
  buildFile,
  createJssgBatchDispatch,
  createNapiJssgEngine,
  dynamic,
  executeJssgCommand,
  jssg,
  parallel,
  run,
  spawnBridge,
  type JssgArtifact,
  type JssgDispatch,
  type JssgOperation,
  type OperationCompletion,
  type Selector,
} from "../src/index.ts";

const bin =
  process.env.CODEMOD_BRIDGE_BIN ??
  resolve(import.meta.dirname, "../../../target/debug/butterflow-execution-bridge");
const fixtures = resolve(import.meta.dirname, "fixtures");
const cli = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");
const benchmarkTransforms = resolve(import.meta.dirname, "../../../benchmarks/runtimes/transforms");

beforeAll(() => {
  if (!existsSync(bin)) {
    throw new Error(
      `bridge binary not found at ${bin}; run 'cargo build -p butterflow-execution-bridge'`,
    );
  }
  if (!isNapiAvailable()) {
    throw new Error(
      "jssg napi addon not built; run 'pnpm --filter @codemod.com/jssg-napi build:debug'",
    );
  }
});

let artifacts: Map<string, JssgArtifact>;
beforeAll(() => {
  const benchmark = join(mkdtempSync(join(tmpdir(), "codemod-bench-wf-")), "bench.ts");
  writeFileSync(
    benchmark,
    `import { jssg } from ${JSON.stringify(resolve(import.meta.dirname, "../src/index.ts"))};
import consoleToLogger from ${JSON.stringify(join(benchmarkTransforms, "console-to-logger.ts"))};
import renameVitestImport from ${JSON.stringify(join(benchmarkTransforms, "rename-vitest-import.ts"))};
export const a = jssg({ name: "console-to-logger", language: "typescript", transform: consoleToLogger });
export const b = jssg({ name: "rename-vitest-import", language: "typescript", transform: renameVitestImport });
`,
  );
  artifacts = new Map(
    [
      ...buildFile(join(fixtures, "jssg/transforms.ts")).artifacts,
      ...buildFile(join(fixtures, "jssg/inprocess.ts")).artifacts,
      ...buildFile(benchmark).artifacts,
    ].map((a) => [a.hash, a]),
  );
  rmSync(dirname(benchmark), { recursive: true, force: true });
});

const repos: string[] = [];
afterEach(() => {
  for (const repo of repos.splice(0)) rmSync(repo, { recursive: true, force: true });
});

function makeRepo(files: Record<string, string>): string {
  const repo = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-parity-")));
  repos.push(repo);
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(repo, path)), { recursive: true });
    writeFileSync(join(repo, path), content);
  }
  return repo;
}

function tree(root: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const name of readdirSync(root, { recursive: true, encoding: "utf8" }).sort()) {
    const path = join(root, name);
    try {
      out[name] = readFileSync(path, "utf8");
    } catch {
      // directory
    }
  }
  return out;
}

function artifactRef(name: string) {
  const artifact = [...artifacts.values()].find((a) => a.name === name);
  if (!artifact) throw new Error(`no fixture transform named ${name}`);
  return { name: artifact.name, hash: artifact.hash };
}

interface Arm {
  batch: OperationCompletion | undefined;
  completion: OperationCompletion;
  tree: Record<string, string>;
}

type Spec = Omit<JssgOperation, "kind" | "transform" | "language"> & {
  transform: string;
  language?: string;
  selector?: Selector;
};

/** Run one JSSG command on both runtimes against copies of `files`. */
async function both(
  spec: Spec,
  files: Record<string, string>,
): Promise<Record<"bridge" | "node", Arm>> {
  const { transform, language = "typescript", ...rest } = spec;
  const operation: JssgOperation = {
    kind: "jssg",
    transform: artifactRef(transform),
    language,
    ...rest,
  };
  const arms = {} as Record<"bridge" | "node", Arm>;
  for (const runtime of ["bridge", "node"] as const) {
    const repo = makeRepo(files);
    const inner: JssgDispatch =
      runtime === "bridge"
        ? (request, signal) => spawnBridge({ bin, cwd: repo }, request, signal)
        : createJssgBatchDispatch({ engine: createNapiJssgEngine() });
    let batch: OperationCompletion | undefined;
    const completion = await executeJssgCommand({
      cwd: repo,
      artifacts,
      commandId: "parity",
      operation,
      globalExcludes: null,
      dispatch: async (request, signal) => (batch = await inner(request, signal)),
    });
    arms[runtime] = { batch, completion, tree: tree(repo) };
  }
  return arms;
}

/** Same batch result, same completion, same files; messages compared loosely on failure. */
function expectParity(arms: Record<"bridge" | "node", Arm>, status: OperationCompletion["status"]) {
  const { bridge, node } = arms;
  expect(bridge.completion.status, JSON.stringify(bridge.completion)).toBe(status);
  expect(node.completion.status, JSON.stringify(node.completion)).toBe(status);
  expect(node.tree).toEqual(bridge.tree);
  expect(node.batch?.status).toEqual(bridge.batch?.status);
  if (status === "succeeded") {
    expect(node.batch).toEqual(bridge.batch);
    expect(node.completion).toEqual(bridge.completion);
  } else {
    expect(node.completion.error?.details).toEqual(bridge.completion.error?.details);
  }
}

describe("node runtime parity with the bridge", () => {
  it("structured results, relative filenames, and helpers bundled into the artifact", async () => {
    const arms = await both(
      { transform: "replace" },
      { "a.ts": "oldApi(1);\n", "nested/b.ts": "oldApi(2);\nplain();\n", "c.ts": "plain();\n" },
    );
    expectParity(arms, "succeeded");
    expect(arms.node.tree["nested/b.ts"]).toBe("newApi(2);\nplain();\n");
  });

  it("the static selector skips files before any transform runs", async () => {
    const files = { "a.ts": "oldApi('a');\n", "plain.ts": "plain();\n" };
    expectParity(
      await both({ transform: "guarded", selector: { rule: { pattern: "oldApi($A)" } } }, files),
      "succeeded",
    );
    const unguarded = await both({ transform: "guarded" }, files);
    expectParity(unguarded, "failed");
    expect(unguarded.node.completion.error?.message).toContain("plain.ts");
    expect(unguarded.node.completion.error?.message).toContain("ran on a non-matching file");
  });

  it("nothing is written when a later transform fails", async () => {
    const arms = await both(
      { transform: "fail-second" },
      { "a.ts": "oldApi(1);\n", "b.ts": "oldApi(2);\n" },
    );
    expectParity(arms, "failed");
    expect(arms.node.completion.error?.message).toContain("second file exploded");
  });

  it("conflicting renames, renames with content, and renames returning null", async () => {
    expectParity(
      await both({ transform: "conflict" }, { "a.ts": "a;\n", "b.ts": "b;\n" }),
      "failed",
    );
    const files = { "x.old.ts": "oldApi(1);\n", "y.ts": "oldApi(2);\n", "sub/z.old.ts": "z;\n" };
    expectParity(await both({ transform: "rename" }, files), "succeeded");
    const renamed = await both({ transform: "rename-null" }, files);
    expectParity(renamed, "succeeded");
    expect(Object.keys(renamed.node.tree).sort()).toEqual(["sub/z.new.ts", "x.new.ts", "y.ts"]);
  });

  it("a rename outside the target root fails and writes nothing", async () => {
    const arms = await both({ transform: "escape" }, { "a.ts": "a;\n" });
    expectParity(arms, "failed");
    expect(arms.node.completion.error?.message).toContain("outside the target directory");
  });

  it("jssgTransform() and write() are staged as secondary edits", async () => {
    const files = { "main.ts": "main();\n", "other.ts": "oldApi(9);\n", "same.ts": "same;\n" };
    const secondary = await both({ transform: "secondary", include: ["main.ts"] }, files);
    expectParity(secondary, "succeeded");
    expect(secondary.node.tree["other.ts"]).toBe("newApi(9);\n");
    const written = await both({ transform: "write-other", include: ["main.ts"] }, files);
    expectParity(written, "succeeded");
    expect(written.node.tree["other.ts"]).toBe("export const written = 1;\n");
  });

  it("transform options, invocation input, parse(), and kind()", async () => {
    const files = { "a.ts": "oldApi('x');\n", "dir/b.ts": 'const s = "y";\n' };
    expectParity(
      await both({ transform: "options", input: { n: 1, s: "two" } }, files),
      "succeeded",
    );
    expectParity(await both({ transform: "options" }, files), "succeeded");
    expectParity(await both({ transform: "parse-api" }, files), "succeeded");
  });

  it("invalid results fail the same way", async () => {
    expectParity(await both({ transform: "bad-result" }, { "a.ts": "a;\n" }), "failed");
    expectParity(
      await both(
        { transform: "replace", selector: { rule: 5 } as unknown as Selector },
        { "a.ts": "a;\n" },
      ),
      "failed",
    );
    expectParity(
      await both({ transform: "replace", language: "klingon" }, { "a.ts": "a;\n" }),
      "failed",
    );
  });

  it("the benchmark transforms", async () => {
    const files = {
      "a.ts":
        'import { describe, expect } from \'vitest\';\nimport { defineConfig } from "vitest/config";\nconsole.log("a", console.log(1));\n',
      "b.mjs": 'import type { Foo } from "vitest";\nconsole.log(`x`)\n',
      "c.ts": "export const plain = 1;\n",
    };
    for (const transform of ["console-to-logger", "rename-vitest-import"]) {
      expectParity(
        await both({ transform, include: ["**/*.{ts,mts,cts,js,mjs,cjs}"] }, files),
        "succeeded",
      );
    }
  });
});

describe("node runtime: runs", () => {
  it("evaluates an artifact module once per run (module-level state is per run, not per file)", async () => {
    const counter = jssg({
      name: "counter",
      language: "typescript",
      transform: artifactRef("counter"),
    });
    for (let attempt = 0; attempt < 2; attempt++) {
      const repo = makeRepo({ "a.ts": "a;\n", "b.ts": "b;\n", "c.ts": "c;\n" });
      const executor = new NodeExecutor({ cwd: repo, artifacts });
      const body = dynamic(async () => [
        await counter({ id: "first" }),
        await counter({ id: "second" }),
      ]);
      const { output } = await run(body, {
        executor,
        runtime: NODE_RUNTIME,
      });
      expect(output).toEqual([
        [
          { file: "a.ts", call: 1 },
          { file: "b.ts", call: 2 },
          { file: "c.ts", call: 3 },
        ],
        [
          { file: "a.ts", call: 4 },
          { file: "b.ts", call: 5 },
          { file: "c.ts", call: 6 },
        ],
      ]);
    }
  });

  it("parallel JSSG commands with async transforms keep their staged edits apart", async () => {
    const repo = makeRepo({
      "alpha/main.ts": "main();\n",
      "alpha/other.ts": "oldApi('alpha');\n",
      "alpha/same.ts": "same;\n",
      "beta/main.ts": "main();\n",
      "beta/other.ts": "oldApi('beta');\n",
      "beta/same.ts": "same;\n",
    });
    const secondary = jssg({
      name: "secondary",
      language: "typescript",
      include: ["**/main.ts"],
      transform: artifactRef("secondary"),
    });
    const { output } = await run(
      dynamic(() =>
        parallel(["alpha", "beta"].map((area) => secondary({ id: area, target: { root: area } }))),
      ),
      { executor: new NodeExecutor({ cwd: repo, artifacts }), runtime: NODE_RUNTIME },
    );
    expect(output).toEqual([
      [{ file: "main.ts", changed: "newApi('alpha');\n", unchanged: null }],
      [{ file: "main.ts", changed: "newApi('beta');\n", unchanged: null }],
    ]);
    expect(readFileSync(join(repo, "alpha/other.ts"), "utf8")).toBe("newApi('alpha');\n");
    expect(readFileSync(join(repo, "beta/other.ts"), "utf8")).toBe("newApi('beta');\n");
  });

  it("refuses semantic analysis issued inside dynamic() with nothing executed", async () => {
    const repo = makeRepo({ "a.ts": "a;\n" });
    const semantic = jssg({
      name: "semantic",
      language: "typescript",
      semanticAnalysis: "workspace",
      transform: artifactRef("semantic"),
    });
    const history = new MemoryHistoryStore();
    await expect(
      run(
        dynamic(() => semantic()),
        {
          executor: new NodeExecutor({ cwd: repo, artifacts }),
          runtime: NODE_RUNTIME,
          history,
        },
      ),
    ).rejects.toThrow("JSSG semantic analysis is unsupported in runtime 'node'");
  });

  // Through the CLI: loading a workflow file needs Node's module hooks, which
  // vitest's module runner bypasses.
  it("a shell + jssg workflow file gives the same output and files on both runtimes", () => {
    const workflow = join(fixtures, "node/workflow.ts");
    const files = {
      "src/a.ts": "oldApi(1);\n",
      "src/b.ts": "plain();\n",
      "lib/c.ts": "oldApi(2);\n",
    };
    const results: Record<string, { output: unknown; tree: Record<string, string> }> = {};
    for (const runtime of ["bridge", "node"]) {
      const repo = makeRepo(files);
      const result = spawnSync(
        process.execPath,
        [cli, "run", workflow, `--runtime=${runtime}`, "--target", repo, "--bridge", bin],
        { encoding: "utf8" },
      );
      expect(result.status, result.stderr).toBe(0);
      results[runtime] = { output: JSON.parse(result.stdout), tree: tree(repo) };
    }
    expect(results.node).toEqual(results.bridge);
    expect(results.node!.output).toEqual(["src/a.ts", "src/seeded.ts"]);
    expect(results.node!.tree["lib/c.ts"]).toBe("oldApi(2);\n");
  });
});

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { addonPath } from "../runtime/addon.mjs";
import { assertTree, fixtures, runWorkflow as run } from "../testing/index.mjs";

const require = createRequire(import.meta.url);
const binary = process.env.CODEMOD_BINARY ?? require("../codemod").findBinaryPath().binaryPath;
let addonFound = true;
try {
  addonPath();
} catch {
  addonFound = false;
}
// Running workflows needs a built CLI (`cargo build -p codemod`) for the
// QuickJS host, and the addon (`cargo build -p codemod-node`) for Node's.
const HOSTS = {
  node: { skip: addonFound ? false : "no codemod addon; run cargo build -p codemod-node" },
  quickjs: { skip: binary ? false : "no codemod binary; run cargo build -p codemod" },
};

const PACKAGE = fileURLToPath(new URL("..", import.meta.url));

/**
 * A directory with these files. One holding a workflow also gets this
 * package installed as `codemod`, as a codemod package's own install would.
 */
function scratch(files) {
  const dir = mkdtempSync(join(tmpdir(), "codemod-testing-"));
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), text);
  }
  if ("workflow.ts" in files) {
    mkdirSync(join(dir, "node_modules"), { recursive: true });
    symlinkSync(PACKAGE, join(dir, "node_modules", "codemod"), "dir");
  }
  return dir;
}

const WORKFLOW = `
import { step, forEachFile } from "codemod/workflow";
export default async function main({ params }: { params: { to?: string } }) {
  const found = await step("Find", forEachFile("**/*.ts", (file) => ({ path: file.path })));
  console.log("found", found.outputs.length);
  await step("Rewrite", forEachFile("**/*.ts", (file) => {
    file.write(file.text.replace("old", params.to ?? "new"));
  }));
  await step("Rename", forEachFile("**/*.ts", (file) => file.rename(file.path.replace(/\\.ts$/, ".mts"))));
  return { found: found.outputs.map((o: { path: string }) => o.path) };
}
`;

function workflow() {
  return join(scratch({ "workflow.ts": WORKFLOW }), "workflow.ts");
}

// ------------------------------------------------------------ fixtures ---

test("fixtures finds both layouts, nested groups and params", () => {
  const dir = scratch({
    "dirs/input/a.ts": "a",
    "dirs/expected/a.ts": "a",
    "dirs/params.json": '{ "to": "x" }',
    "file/input.tsx": "a",
    "file/expected.tsx": "a",
    "group/inner/input.ts": "a",
    "dirs/input/nested/input.ts": "not a fixture",
  });

  const found = fixtures(dir);

  assert.deepEqual(
    found.map((f) => f.name),
    ["dirs", "file", "group/inner"],
  );
  assert.deepEqual(found[0].params, { to: "x" });
  assert.equal(found[1].input, join(dir, "file", "input.tsx"));
  assert.equal(found[1].expected, join(dir, "file", "expected.tsx"));
});

test("the fixtures directory can be a fixture itself", () => {
  const dir = scratch({
    "tests/input/a.ts": "a",
    "tests/expected/a.ts": "a",
    "tests/other/input.ts": "a",
  });

  assert.deepEqual(
    fixtures(join(dir, "tests")).map((f) => f.name),
    ["tests", "other"],
  );
});

// ---------------------------------------------------------- assertTree ---

test("assertTree passes on equal trees and ignores line endings", () => {
  assertTree({ "a.ts": "x\r\ny\n" }, { "a.ts": "x\ny\n" });
});

test("assertTree reports one changed file with its own text as actual and expected", () => {
  const error = assert.throws(
    () => assertTree({ "a.ts": "one\ntwo\n" }, { "a.ts": "one\nthree\n" }),
    (error) => {
      assert.equal(error.name, "AssertionError");
      assert.equal(error.actual, "one\ntwo\n");
      assert.equal(error.expected, "one\nthree\n");
      assert.match(error.message, /a\.ts differs/);
      assert.match(error.message, /-three\n\+two/);
      return true;
    },
  );
  void error;
});

test("assertTree reports missing and unexpected files", () => {
  assert.throws(
    () => assertTree({ "b.ts": "b" }, { "a.ts": "a" }),
    (error) => {
      assert.match(error.message, /2 files differ/);
      assert.match(error.message, /missing a\.ts/);
      assert.match(error.message, /unexpected b\.ts/);
      return true;
    },
  );
});

test("assertTree can ignore trailing whitespace", () => {
  assertTree({ "a.ts": "x  \n\n" }, { "a.ts": "x" }, { ignoreWhitespace: true });
});

test("CODEMOD_UPDATE_FIXTURES rewrites the expected directory and file", () => {
  const dir = scratch({ "expected/old.ts": "old", "expected.ts": "old" });
  process.env.CODEMOD_UPDATE_FIXTURES = "1";
  try {
    assertTree({ "new.ts": "new\n" }, join(dir, "expected"));
    assertTree({ "input.ts": "one\n" }, join(dir, "expected.ts"));
  } finally {
    delete process.env.CODEMOD_UPDATE_FIXTURES;
  }
  assertTree(join(dir, "expected"), { "new.ts": "new\n" });
  assert.equal(readFileSync(join(dir, "expected.ts"), "utf8"), "one\n");
});

// --------------------------------------------------------- runWorkflow ---

// The same tests on both hosts: Node (in this process) and the engine's
// QuickJS runtime (through the CLI).
for (const host of Object.keys(HOSTS)) {
  const runWorkflow = (workflow, options) => run(workflow, { ...options, host });

  test(
    `${host}: runWorkflow runs the whole workflow on a copy of the target`,
    HOSTS[host],
    async () => {
      const target = scratch({ "src/a.ts": "const a = old;\n" });

      const run = await runWorkflow(workflow(), { target, params: { to: "fresh" } });

      assert.equal(run.status, "succeeded");
      assert.deepEqual(run.output, { found: ["src/a.ts"] });
      assertTree(run, { "src/a.mts": "const a = fresh;\n" });
      assert.deepEqual(run.changes, { "src/a.mts": "const a = fresh;\n", "src/a.ts": null });
      assert.deepEqual(
        run.steps.map((s) => s.label),
        ["Find", "Rewrite", "Rename"],
      );
      assert.deepEqual(run.logs, [{ step: null, line: "found 1" }]);
      // The target itself is untouched.
      assert.equal(readFileSync(join(target, "src/a.ts"), "utf8"), "const a = old;\n");
    },
  );

  test(
    `${host}: runWorkflow takes inline files and stops after the until step`,
    HOSTS[host],
    async () => {
      const run = await runWorkflow(workflow(), {
        target: { "a.ts": "old\n" },
        until: "Rewrite",
      });

      assert.equal(run.status, "stopped");
      assert.equal(run.stoppedAt, "Rewrite");
      assertTree(run, { "a.ts": "new\n" });
      assert.match(run.diff(), /-old\n\+new/);
    },
  );

  test(`${host}: given replaces a step's result without running it`, HOSTS[host], async () => {
    const run = await runWorkflow(workflow(), {
      target: { "a.ts": "old\n" },
      given: { Find: { outputs: [{ path: "given.ts" }] } },
    });

    assert.deepEqual(run.output, { found: ["given.ts"] });
    assert.equal(run.step("Find").given, true);
    assert.equal(run.step("Rewrite").given, false);
  });

  test(`${host}: labels that match no step are errors`, HOSTS[host], async () => {
    await assert.rejects(
      runWorkflow(workflow(), { target: { "a.ts": "" }, until: "Nope" }),
      /until: no step is labeled "Nope"; the run's steps are: "Find", "Rewrite", "Rename"/,
    );
    await assert.rejects(
      runWorkflow(workflow(), { target: { "a.ts": "" }, given: { Nope: null } }),
      /given: no step is labeled "Nope"/,
    );
  });

  test(
    `${host}: assertTree fails with the run's error when the run failed`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "workflow.ts": `
  import { step } from "codemod/workflow";
  export default async function main() {
    await step("Boom", () => { throw new Error("kaput"); });
  }
  `,
        }),
        "workflow.ts",
      );

      const run = await runWorkflow(path, { target: {} });

      assert.equal(run.status, "failed");
      assert.throws(() => assertTree(run, {}), /the run failed: .*kaput/);
    },
  );

  test(`${host}: a workflow that does not build rejects`, HOSTS[host], async () => {
    const path = join(
      scratch({ "workflow.ts": "export default async function main( {" }),
      "workflow.ts",
    );
    await assert.rejects(runWorkflow(path, { target: {} }), /cannot run .*workflow\.ts/);
  });

  test(`${host}: fixtures and runWorkflow work together`, HOSTS[host], async () => {
    const dir = scratch({
      "basic/input.ts": "old\n",
      "basic/expected.ts": "new\n",
      "params/input/a.ts": "old\n",
      "params/expected/a.mts": "custom\n",
      "params/params.json": '{ "to": "custom" }',
    });
    const wf = workflow();
    for (const fixture of fixtures(dir)) {
      const run = await runWorkflow(wf, {
        target: fixture.input,
        params: fixture.params,
        until: fixture.name === "basic" ? "Rewrite" : undefined,
      });
      assertTree(run, fixture.expected);
    }
  });

  test(
    `${host}: state set in callbacks reaches later steps and the body`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "workflow.ts": `
import { step, forEachFile, getState, setState, unsetState } from "codemod/workflow";
export default async function main() {
  setState("seen", []);
  setState("drop", true);
  await step("Collect", forEachFile("**/*.ts", (file) => {
    setState("last", file.path);
    setState("scratch", 1, false);
    unsetState("drop");
  }));
  const fromStep = await step("Read", () => ({ last: getState("last") ?? null, scratch: getState("scratch") ?? null }));
  return { last: getState("last"), drop: getState("drop") ?? null, scratch: getState("scratch") ?? null, fromStep };
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, { target: { "a.ts": "a\n" } });
      assert.equal(result.status, "succeeded", result.error);
      assert.deepEqual(result.output, {
        last: "a.ts",
        drop: null,
        scratch: null,
        fromStep: { last: "a.ts", scratch: null },
      });
    },
  );

  test(`${host}: the older codemod: spelling imports the same modules`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile } from "codemod:workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  const found = await step(forEachFile("**/*.ts", (file) => parse(file).root().findAll("const $A = $B").length));
  return found.outputs;
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, { target: { "a.ts": "const a = 1;\n" } });
    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(result.output, [1]);
  });

  // What follows used to differ between the hosts.

  test(`${host}: state in a step is copied in and out, as JSON keeps it`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, getState, setState } from "codemod/workflow";
export default async function main() {
  setState("seen", ["a"]);
  setState("same", 1);
  const inStep = await step("Edit", () => {
    const seen = getState("seen");
    seen.push("b");
    setState("seen", seen);
    const kept = { n: 1 };
    setState("kept", kept);
    kept.n = 2;
    setState("odd", { at: new Date(0), missing: undefined, big: Number.NaN });
    setState("same", 1);
    return Object.keys(getState()).sort();
  });
  return { inStep, seen: getState("seen"), kept: getState("kept"), odd: getState("odd") };
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, { target: { "a.ts": "a\n" } });
    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(result.output, {
      inStep: ["kept", "odd", "same", "seen"],
      seen: ["a", "b"],
      kept: { n: 1 },
      odd: { at: {}, missing: null, big: null },
    });
  });

  test(`${host}: a failing until step fails the run`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step } from "codemod/workflow";
export default async function main() {
  await step("Boom", () => { throw new Error("no"); });
  await step("After", () => 1);
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, { target: { "a.ts": "a\n" }, until: "Boom" });
    assert.equal(result.status, "failed");
    assert.equal(result.error, "StepError: step 'Boom' failed: no");
    assert.equal(result.ok, false);
  });

  test(`${host}: metrics, runtime logs and reported failures`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import { useMetricAtom } from "codemod/metrics";
import { warn, failFile } from "codemod/runtime";
export default async function main() {
  const found = await step("Scan", forEachFile("**/*.ts", (file) => {
    const metric = useMetricAtom("seen");
    metric.increment({ count: 3 }, 2.9);
    metric.increment({ count: 3 }, -5);
    warn("careful", { file: file.path });
    if (file.path === "b.ts") {
      try { failFile("not this one"); } catch {}
    }
    return file.path;
  }));
  return { outputs: found.outputs, failures: found.failures, metrics: found.metrics };
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, { target: { "a.ts": "a\n", "b.ts": "b\n" } });
    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(result.output.outputs, ["a.ts"]);
    assert.deepEqual(result.output.failures, [{ path: "b.ts", error: "not this one" }]);
    assert.deepEqual(result.output.metrics, {
      seen: [{ cardinality: { count: "3" }, count: 4 }],
    });
    const warnings = result.logs.map((log) => log.line).filter((line) => line.includes("careful"));
    assert.deepEqual(warnings.sort(), [
      '[warn] careful {"file":"a.ts"}',
      '[warn] careful {"file":"b.ts"}',
    ]);
  });

  test(`${host}: a workflow the CLI would not build is refused`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  let count = 0;
  await step(forEachFile("**/*.ts", () => { count++; }));
  return count;
}
`,
      }),
      "workflow.ts",
    );
    await assert.rejects(runWorkflow(path, { target: { "a.ts": "a\n" } }), /assigns to 'count'/);
  });

  test(
    `${host}: the body's lock and step outputs are there, and a function result is null`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "workflow.ts": `
import { acquireLock, getOrSetStepOutput, getStepOutput, setStepOutput } from "codemod/workflow";
export default async function main() {
  const release = acquireLock("x");
  release();
  setStepOutput("a", "1");
  return [getStepOutput("", "a"), getOrSetStepOutput("", "b", "2"), () => 1];
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, { target: { "a.ts": "a\n" } });
      assert.equal(result.status, "succeeded", result.error);
      assert.deepEqual(result.output, ["1", "2", null]);
    },
  );

  test(`${host}: an aborted run ends as canceled`, HOSTS[host], async () => {
    const result = await runWorkflow(workflow(), {
      target: { "a.ts": "old\n" },
      signal: AbortSignal.abort(),
    });
    assert.equal(result.status, "canceled");
    assert.deepEqual(result.files, { "a.ts": "old\n" });
  });
}

// ------------------------------------------------------------ node host ---

test(
  "node: callbacks parse with codemod/ast-grep, and their console output is logged by step",
  HOSTS.node,
  async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
export default async function main() {
  const found = await step("Consts", forEachFile("**/*.ts", (file) => {
    const names = parse(file).root().findAll("const $A = $B").map((n) => n.getMatch("A")!.text());
    console.log(file.path, names.length);
    return names;
  }));
  return found.outputs;
}
`,
      }),
      "workflow.ts",
    );

    const result = await run(path, { target: { "a.ts": "const x = 1;\nconst y = 2;\n" } });

    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(result.output, [["x", "y"]]);
    assert.deepEqual(result.logs, [{ step: "Consts", line: "a.ts 2" }]);
  },
);

test("node: runWorkflow takes the workflow module itself", HOSTS.node, async () => {
  const path = workflow();
  const module = await import(pathToFileURL(path).href);

  const result = await run(module, { path, target: { "a.ts": "old\n" } });

  assertTree(result, { "a.mts": "new\n" });
});

test("node: the orchestrator copy matches the engine's", () => {
  const source = readFileSync(
    new URL("../../../workflow/src/js/orchestrator.js", import.meta.url),
    "utf8",
  );
  const copy = readFileSync(new URL("../runtime/orchestrator.mjs", import.meta.url), "utf8");
  assert.ok(
    copy.endsWith(source),
    "runtime/orchestrator.mjs is out of date; run pnpm sync-orchestrator",
  );
});

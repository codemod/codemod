import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
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
  if (Object.keys(files).some((path) => path.endsWith("workflow.ts"))) {
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
test(
  "node: a jssg reducer that calls failStep fails the step even if it catches it",
  HOSTS.node,
  async () => {
    const path = join(
      scratch({
        "transform.ts": `
export default function transform(root: any) {
  return { edit: "const edited = true;\\n", data: 1 };
}
`,
        "workflow.ts": `
import { step, jssg } from "codemod/workflow";
import transform from "./transform.ts";
import { failStep } from "codemod/runtime";
export default async function main() {
  return (await step("Count", jssg(transform, {
    reduce: (total, { data }) => {
      try { failStep("cannot add"); } catch {}
      return total + data;
    },
    initial: 0,
  }))).data;
}
`,
      }),
      "workflow.ts",
    );
    const result = await run(path, { target: { "a.ts": "a\n" }, host: "node" });
    assert.equal(result.status, "failed");
    assert.match(result.error, /cannot add/);
    assert.deepEqual(result.files, { "a.ts": "a\n" });
  },
);

test(
  "node: a workflow module imported without the host gives jssg the transform's code",
  HOSTS.node,
  async () => {
    const path = join(
      scratch({
        "transform.ts": "export default function transform() { return null; }\n",
        "workflow.ts": `
import { step, jssg } from "codemod/workflow";
import transform from "./transform.ts";
export default async function main() {
  await step(jssg(transform));
}
`,
      }),
      "workflow.ts",
    );
    // Imported by the test itself, so the host's load hook does not see it.
    const module = await import(pathToFileURL(path).href);
    const result = await run(module, { path, target: { "a.ts": "a\n" }, host: "node" });
    assert.equal(result.status, "failed");
    assert.match(result.error, /jssg was given the transform's code/);
  },
);

test("node: a jssg reducer that runs out of time folds no more files", HOSTS.node, async () => {
  const path = join(
    scratch({
      "transform.ts": `
export default function transform(root: any) {
  return { edit: "const edited = true;\\n", data: 1 };
}
`,
      "workflow.ts": `
import { step, jssg, sleep } from "codemod/workflow";
import transform from "./transform.ts";
const seen = [];
export default async function main() {
  try {
    await step("Slow", jssg(transform, {
      reduce: async (total, { file }) => {
        seen.push(file);
        await new Promise((done) => setTimeout(done, 300));
        return total + 1;
      },
      initial: 0,
    }), { timeout: 100 });
  } catch {}
  await sleep("500ms");
  return seen;
}
`,
    }),
    "workflow.ts",
  );
  const result = await run(path, { target: { "a.ts": "a\n", "b.ts": "b\n" }, host: "node" });
  assert.equal(result.status, "succeeded", result.error);
  assert.match(result.steps.find(({ label }) => label === "Slow")?.error, /timed out/);
  assert.deepEqual(result.output, ["a.ts"]);
  assert.deepEqual(result.files, { "a.ts": "a\n", "b.ts": "b\n" });
});

test("node: a retried jssg reducer starts from its initial value again", HOSTS.node, async () => {
  const path = join(
    scratch({
      "transform.ts": `
export default function transform(root: any) {
  const original = root.root().text();
  return { edit: original + "// edited once\\n", data: original.length };
}
`,
      "workflow.ts": `
import { step, jssg } from "codemod/workflow";
import transform from "./transform.ts";
const tries = [];
export default async function main() {
  const seen = await step("Seen", jssg(transform, {
    reduce: (files, { file, data }) => {
      files.push({ file, data });
      tries.push(file);
      if (tries.length === 1) throw new Error("once");
      return files;
    },
    initial: [],
  }), { retries: 1 });
  return seen.data;
}
`,
    }),
    "workflow.ts",
  );
  const result = await run(path, { target: { "a.ts": "a\n" }, host: "node" });
  assert.equal(result.status, "succeeded", result.error);
  assert.deepEqual(result.output, [{ file: "a.ts", data: 2 }]);
  assert.deepEqual(result.files, { "a.ts": "a\n// edited once\n" });
});

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

  test(`${host}: the workflow context has the execution platform`, HOSTS[host], async () => {
    const dir = scratch({
      "workflow.ts": `export default function main({ platform }) { return platform; }`,
    });
    const result = await run(join(dir, "workflow.ts"), { target: {}, host });
    assert.equal(result.output, process.platform);
  });

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

  test(`${host}: workflows have no state functions`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { getState } from "codemod/workflow";
export default async function main() {
  return getState("x");
}
`,
      }),
      "workflow.ts",
    );
    // The engine says what replaced it; Node.js only that the export is gone.
    await assert.rejects(
      runWorkflow(path, { target: { "a.ts": "a\n" } }),
      /'getState' was removed|export named 'getState'/,
    );
  });

  test(
    `${host}: a jssg step's data is a sorted list, or what its reducer folds`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "transform.ts": `
export default function transform(root: any) {
  return { data: root.root().text().length };
}
`,
          "workflow.ts": `
import { step, jssg } from "codemod/workflow";
import transform from "./transform.ts";
export default async function main() {
  const listed = await step("List", jssg(transform));
  const summed = await step("Sum", jssg(transform, {
    reduce: (total, { data }) => total + data,
    initial: 0,
  }));
  return { listed: listed.data, summed: summed.data };
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, { target: { "b.ts": "bb\n", "a.ts": "a\n" } });
      assert.equal(result.status, "succeeded", result.error);
      assert.deepEqual(result.output, {
        listed: [
          { file: "a.ts", data: 2 },
          { file: "b.ts", data: 3 },
        ],
        summed: 5,
      });
    },
  );

  test(
    `${host}: an imported transform runs on the files, and the body does not load it`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          // Marks the process that evaluates it: the transform runs in the
          // engine's own runtime, never in the workflow body.
          "scripts/codemod.ts": `
(globalThis as any).__codemodTransformLoaded = true;
export const KEYWORD = "let";
export default function transform(root: any, options: any) {
  return root.root().text().replace("var", options.params.keyword);
}
`,
          "workflow.ts": `
import { step, forEachFile, jssg } from "codemod/workflow";
import codemod from "./scripts/codemod.ts";
export default async function main() {
  const migrated = await step("Migrate", forEachFile("**/*.ts", jssg(codemod, { params: { keyword: "let" } })));
  return migrated.changes.written;
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, { target: { "a.ts": "var a = 1;\n" } });
      assert.equal(result.status, "succeeded", result.error);
      assert.deepEqual(result.output, ["a.ts"]);
      assert.deepEqual(result.files, { "a.ts": "let a = 1;\n" });
      assert.equal(globalThis.__codemodTransformLoaded, undefined);
    },
  );

  test(`${host}: a path given to jssg does not build`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "scripts/codemod.ts": "export default () => null;\n",
        "workflow.ts": `
import { step, jssg } from "codemod/workflow";
export default async function main() {
  await step(jssg("./scripts/codemod.ts"));
}
`,
      }),
      "workflow.ts",
    );
    await assert.rejects(
      runWorkflow(path, { target: { "a.ts": "a\n" } }),
      /jssg takes an imported transform, not a file path: add import codemod from/,
    );
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

  test(
    `${host}: a workflow in src/ runs its scripts from import.meta.dirname`,
    HOSTS[host],
    async () => {
      const dir = scratch({
        "codemod.yaml": 'schema_version: "1.0"\nname: p\nversion: 0.1.0\n',
        "src/scripts/echo.sh": `printf '%s|' "$@"\n`,
        "src/workflow.ts": `
import { step, shell } from "codemod/workflow";
export default async function main() {
  const echoed = await step("Echo", shell\`sh \${import.meta.dirname}/scripts/echo.sh \${"two words"} \${["it's", 3]}\`);
  return { dirname: import.meta.dirname, output: echoed.stdout };
}
`,
      });
      const result = await runWorkflow(join(dir, "src", "workflow.ts"), { target: {} });
      assert.equal(result.status, "succeeded", result.error);
      assert.equal(realpathSync(result.output.dirname), realpathSync(join(dir, "src")));
      assert.match(result.output.output, /two words\|it's\|3\|/);
    },
  );

  test(`${host}: runtime logs and reported failures`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import { warn, failFile } from "codemod/runtime";
export default async function main() {
  const found = await step("Scan", forEachFile("**/*.ts", (file) => {
    warn("careful", { file: file.path });
    if (file.path === "b.ts") {
      try { failFile("not this one"); } catch {}
    }
    return file.path;
  }));
  return { outputs: found.outputs, failures: found.failures };
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, { target: { "a.ts": "a\n", "b.ts": "b\n" } });
    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(result.output.outputs, ["a.ts"]);
    assert.deepEqual(result.output.failures, [{ path: "b.ts", error: "not this one" }]);
    // Some files failed: the step finished, as partial.
    assert.equal(result.step("Scan").status, "partial");
    assert.equal(result.step("Scan").error, "1 of 2 files failed");
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

  test(`${host}: a function in the body's result becomes null`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
export default async function main() {
  return [1, () => 1];
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, { target: { "a.ts": "a\n" } });
    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(result.output, [1, null]);
  });

  test(`${host}: an aborted run ends as canceled`, HOSTS[host], async () => {
    const result = await runWorkflow(workflow(), {
      target: { "a.ts": "old\n" },
      signal: AbortSignal.abort(),
    });
    assert.equal(result.status, "canceled");
    assert.deepEqual(result.files, { "a.ts": "old\n" });
  });

  test(
    `${host}: steps retry and time out, sleep waits, and given stands in for an event`,
    HOSTS[host],
    async () => {
      const source = `
import { step, sleep, waitForEvent } from "codemod/workflow";
export default async function main() {
  await sleep("50ms");
  const event = await waitForEvent("go", { timeout: "1m" });
  try {
    await step(
      "Slow",
      async () => { await new Promise((done) => setTimeout(done, 1500)); },
      { timeout: 200 },
    );
  } catch (error) {
    console.log((error as Error).message);
  }
  await step("Fails", () => { console.log("try"); throw new Error(event.word); }, {
    retries: 2,
    retryDelay: 0,
  });
}
`;
      const started = Date.now();
      const result = await runWorkflow(join(scratch({ "workflow.ts": source }), "workflow.ts"), {
        target: {},
        given: { "wait for go": { word: "no" } },
      });

      assert.equal(result.status, "failed");
      assert.match(result.error, /step 'Fails' failed: no/);
      assert.ok(Date.now() - started >= 50);
      assert.deepEqual(
        result.steps.map((s) => [s.label, s.status]),
        [
          ["sleep 50ms", "succeeded"],
          ["wait for go", "succeeded"],
          ["Slow", "failed"],
          ["Fails", "failed"],
        ],
      );
      assert.deepEqual(result.logs, [
        { step: null, line: "step 'Slow' failed: timed out after 200ms" },
        { step: "Fails", line: "try" },
        { step: "Fails", line: "try" },
        { step: "Fails", line: "try" },
      ]);
    },
  );
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

test(
  "node: this package and the addon were built from the same orchestrator",
  HOSTS.node,
  async () => {
    const { addon } = await import("../runtime/addon.mjs");
    const { ORCHESTRATOR_VERSION } = await import("../runtime/orchestrator-version.mjs");
    assert.equal(
      addon().orchestratorVersion(),
      ORCHESTRATOR_VERSION,
      "rebuild both: cargo build -p codemod-node, and pnpm --filter codemod build",
    );
  },
);

test("the built orchestrator has no types left in it", () => {
  const built = readFileSync(new URL("../runtime/orchestrator.mjs", import.meta.url), "utf8");
  assert.doesNotMatch(built, /^(export )?(interface|type) /m);
});

test(
  "node: a workflow importing the dependency APIs loads, and using one says to run it with the CLI",
  HOSTS.node,
  async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step } from "codemod/workflow";
import { dependencyGraph, registry, semver } from "codemod/deps";
export default async function main() {
  void registry;
  void semver;
  await step("Graph", dependencyGraph({}, (g) => g.workspaces().length));
}
`,
      }),
      "workflow.ts",
    );

    const result = await run(path, { target: {} });

    assert.equal(result.status, "failed");
    assert.match(result.error ?? "", /dependencyGraph\(\) is not available on the Node\.js host/);
  },
);

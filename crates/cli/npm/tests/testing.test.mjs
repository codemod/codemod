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
import { assertTree, fixtures, runCallback, runWorkflow as run } from "../testing/index.mjs";

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

test(
  "node: a workflow module imported without the host cannot give shard a function",
  HOSTS.node,
  async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, shard } from "codemod/workflow";
export default async function main() {
  await step(shard({ method: { function: ({ files }) => [] }, files: "**/*.ts" }));
}
`,
      }),
      "workflow.ts",
    );
    // Imported by the test itself, so the host's load hook does not see it.
    const module = await import(pathToFileURL(path).href);
    const result = await run(module, { path, target: { "a.ts": "a\n" }, host: "node" });
    assert.equal(result.status, "failed");
    assert.match(result.error, /shard's method.function was not extracted by the build/);
  },
);

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
    `${host}: an imported callback edits, renames and returns outputs`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "scripts/migrate.ts": `
import { parse } from "codemod/ast-grep";
import { suffix } from "./helpers.ts";
export default function migrate(file: any) {
  if (file.path.endsWith("old.ts")) file.rename(file.path.replace("old.ts", "new.ts"));
  const root = parse(file).root();
  const found = root.findAll({ rule: { pattern: "var $A = $B" } });
  if (!found.length) return undefined;
  file.write(file.text.replace("var ", "let ") + suffix);
  return { path: file.path, count: found.length };
}
`,
          "scripts/helpers.ts": 'export const suffix = "// done\\n";\n',
          "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import migrate from "./scripts/migrate.ts";
export default async function main() {
  const result = await step("Migrate", forEachFile({ include: "**/*.ts", language: "typescript" }, migrate));
  return result.outputs;
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, {
        target: { "a.ts": "var a = 1;\n", "old.ts": "const b = 2;\n" },
      });
      assert.equal(result.status, "succeeded", result.error);
      assert.deepEqual(result.output, [{ path: "a.ts", count: 1 }]);
      assert.deepEqual(result.files, {
        "a.ts": "let a = 1;\n// done\n",
        "new.ts": "const b = 2;\n",
      });
    },
  );

  test(`${host}: shard runs its filter and function, inline or imported`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "scripts/shard.ts": `
export default function shardFiles(input: { files: string[] }) {
  return input.files.map((file, index) => ({ name: "s" + index, _meta_shard: index, _meta_files: [file] }));
}
`,
        "workflow.ts": `
import { step, shard } from "codemod/workflow";
import shardFiles from "./scripts/shard.ts";
export default async function main() {
  const keyword = "var";
  const shards = await step(shard({
    method: { function: shardFiles },
    files: "**/*.ts",
    filter: (file) => file.text.includes(keyword),
  }));
  return shards.map((current: any) => current._meta_files[0]).sort();
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, {
      target: { "a.ts": "var a = 1;\n", "b.ts": "var b = 1;\n", "c.ts": "let c = 1;\n" },
    });
    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(result.output, ["a.ts", "b.ts"]);
  });

  test(`${host}: jssg was removed, and the build says what replaced it`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, jssg } from "codemod/workflow";
export default async function main() {}
`,
      }),
      "workflow.ts",
    );
    await assert.rejects(
      runWorkflow(path, { target: { "a.ts": "a\n" } }),
      /jssg was removed: per-file logic is a forEachFile callback/,
    );
  });

  test(
    `${host}: ast-grep rules run inline, built from params, and imported from YAML`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "rules/config.yml":
            "id: no-var\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n---\nid: no-console\nlanguage: javascript\nrule:\n  pattern: console.log($$$)\nfix: ''\n",
          "workflow.ts": `
import { step, forEachFile, astGrep } from "codemod/workflow";
import rules from "./rules/config.yml";
export default async function main({ params }: { params: { from: string; to: string } }) {
  const imported = await step("Imported", forEachFile("**/*.js", astGrep(rules)));
  const rename = { id: "rename", language: "javascript", rule: { pattern: params.from + "()" }, fix: params.to + "()" };
  await step("Dynamic", forEachFile(astGrep([rename])));
  return { ids: (rules as { id: string }[]).map((rule) => rule.id), matches: imported.matches };
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, {
        target: { "a.js": "var a = 1;\nconsole.log(a);\nold();\n" },
        params: { from: "old", to: "fresh" },
      });
      assert.equal(result.status, "succeeded", result.error);
      assert.deepEqual(result.output, { ids: ["no-var", "no-console"], matches: 2 });
      assert.equal(result.files["a.js"], "let a = 1;\n;\nfresh();\n");
    },
  );

  test(`${host}: a rule file path given to astGrep does not build`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  await step(forEachFile(astGrep("./rules.yml")));
}
`,
      }),
      "workflow.ts",
    );
    await assert.rejects(
      runWorkflow(path, { target: { "a.js": "a\n" } }),
      /astGrep takes rules, not a file path: add import rules from/,
    );
  });

  test(`${host}: astGrep repeats until nothing changes`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  const rule = { id: "log", language: "javascript", rule: { pattern: "console.log($A)" }, fix: "logger.info($A)" };
  const result = await step(forEachFile(astGrep(rule, { repeat: true })));
  return result.matches;
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, {
      target: { "a.js": "console.log(console.log(1));\n" },
    });
    assert.equal(result.status, "succeeded", result.error);
    assert.equal(result.output, 2);
    assert.equal(result.files["a.js"], "logger.info(logger.info(1));\n");
  });

  test(`${host}: astGrep is not a step on its own`, HOSTS[host], async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, astGrep } from "codemod/workflow";
export default async function main() {
  await step(astGrep({ id: "r", language: "javascript", rule: { pattern: "a" } }) as any);
}
`,
      }),
      "workflow.ts",
    );
    const result = await runWorkflow(path, { target: { "a.js": "a\n" } });
    assert.equal(result.status, "failed");
    assert.match(result.error, /astGrep\(\.\.\.\) is not a step on its own/);
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

  test(`${host}: git steps refuse to run alongside other steps`, HOSTS[host], async () => {
    const dir = scratch({
      "workflow.ts": `
import { step, branch, commit } from "codemod/workflow";
export default async function main() {
  await Promise.all(["a", "b"].map(async (name) => {
    await step(branch("codemod-" + name));
    await step(commit("migrate " + name));
  }));
}
`,
    });
    const result = await runWorkflow(join(dir, "workflow.ts"), { target: {} });
    assert.equal(result.status, "failed");
    assert.match(result.error, /managed git steps can't run in parallel/);
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

const CALLBACK = `
import { parse } from "codemod/ast-grep";
export default function migrate(file: any) {
  const root = parse(file).root();
  const found = root.findAll({ rule: { pattern: "var $A = $B" } });
  if (!found.length) return;
  file.write(root.commitEdits(found.map((node: any) => node.replace(node.text().replace(/^var/, "let")))));
  return { count: found.length };
}
`;

test("runCallback runs a callback file on fixtures as jssg test does", HOSTS.quickjs, async () => {
  const dir = scratch({
    "migrate.ts": CALLBACK,
    "tests/one/input.ts": "var a = 1;\n",
    "tests/one/expected.ts": "let a = 1;\n",
    "tests/tree/input/src/a.ts": "var a = 1;\nvar b = 2;\n",
    "tests/tree/input/notes.md": "var x = 1\n",
    "tests/tree/expected/src/a.ts": "let a = 1;\nlet b = 2;\n",
    "tests/tree/expected/notes.md": "var x = 1\n",
  });
  const found = fixtures(join(dir, "tests"));
  assert.equal(found.length, 2);
  for (const fixture of found) {
    const result = await runCallback(join(dir, "migrate.ts"), {
      target: fixture.input,
      language: "typescript",
    });
    assertTree(result, fixture.expected);
  }
  const result = await runCallback(join(dir, "migrate.ts"), {
    target: join(dir, "tests/tree/input"),
    language: "typescript",
  });
  assert.equal(result.status, "succeeded");
  assert.deepEqual(result.outputs, [{ file: "src/a.ts", data: { count: 2 } }]);
  assert.deepEqual(Object.keys(result.changes), ["src/a.ts"]);
});

test("runCallback resolves definitions with semantic analysis", HOSTS.quickjs, async () => {
  const dir = scratch({
    "find.ts": `
import { parse } from "codemod/ast-grep";
export default (file: any) => {
  const use = parse(file).root().find({ rule: { pattern: "console.log($X)" } }).getMatch("X");
  return use.definition()?.node.range().start.line ?? null;
};
`,
  });
  const target = { "a.ts": "const a = 1;\nconsole.log(a);\n" };
  const file = await runCallback(join(dir, "find.ts"), { target, language: "typescript" });
  assert.deepEqual(file.outputs, [{ file: "a.ts", data: 0 }]);
  const none = await runCallback(join(dir, "find.ts"), {
    target,
    language: "typescript",
    semanticAnalysis: "none",
  });
  assert.deepEqual(none.outputs, [{ file: "a.ts", data: null }]);
});

test("runCallback fails the run when the callback fails on a file", HOSTS.quickjs, async () => {
  const dir = scratch({
    "boom.ts": `export default (file: any) => { if (file.path === "b.ts") throw new Error("boom"); file.write("ok\\n"); };`,
  });
  const result = await runCallback(join(dir, "boom.ts"), {
    target: { "a.ts": "a\n", "b.ts": "b\n" },
    language: "typescript",
  });
  assert.equal(result.status, "failed");
  assert.equal(result.ok, false);
  assert.match(result.error ?? "", /b\.ts: .*boom/);
  assert.deepEqual(
    result.failures.map((failure) => failure.path),
    ["b.ts"],
  );
  assert.equal(result.files["a.ts"], "ok\n");
  assert.throws(() => assertTree(result, {}), /the run failed/);
});

test("runCallback applies renames and deletes to the copy", HOSTS.quickjs, async () => {
  const dir = scratch({
    "move.ts": `
export default (file: any) => {
  if (file.path === "old.ts") file.rename("lib/new.ts");
  if (file.path === "gone.ts") file.delete();
};
`,
  });
  const result = await runCallback(join(dir, "move.ts"), {
    target: { "old.ts": "a\n", "gone.ts": "b\n", "kept.ts": "c\n" },
    language: "typescript",
  });
  assert.equal(result.status, "succeeded", result.error);
  assert.deepEqual(result.files, { "kept.ts": "c\n", "lib/new.ts": "a\n" });
});

test("runCallback rejects a callback file that is not there", HOSTS.quickjs, async () => {
  const dir = scratch({});
  await assert.rejects(
    runCallback(join(dir, "missing.ts"), { target: { "a.ts": "a\n" }, language: "typescript" }),
    /no callback file/,
  );
});

test("runCallback needs a target and a language", async () => {
  await assert.rejects(runCallback("migrate.ts", { target: {} }), /language is required/);
  await assert.rejects(runCallback("migrate.ts", {}), /target is required/);
});

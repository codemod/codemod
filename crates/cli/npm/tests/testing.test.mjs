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

const FAILING_CHECK = `
import { fail, step } from "codemod/workflow";
export default async function main() {
  const found = await step("Find", () => ["a.ts"]);
  const report = { checks: [{ id: "no-var", status: "fail", files: found }] };
  if (found.length) fail("1 check failed", { output: report, exitCode: 3 });
  return report;
}
`;

for (const host of ["node", "quickjs"]) {
  test(`${host}: fail() ends the run failed and keeps its output`, HOSTS[host], async () => {
    const path = join(scratch({ "workflow.ts": FAILING_CHECK }), "workflow.ts");
    const result = await run(path, { target: { "a.ts": "var a;\n" }, host });
    assert.equal(result.status, "failed");
    assert.equal(result.ok, false);
    assert.equal(result.error, "1 check failed");
    assert.equal(result.exitCode, 3);
    assert.deepEqual(result.output, {
      checks: [{ id: "no-var", status: "fail", files: ["a.ts"] }],
    });
  });
}

/**
 * A System One API on localhost: yes/no questions get 0.9, choices their
 * first option. Keeps each request's authorization and body.
 */
async function fakeSystemOne() {
  const { createServer } = await import("node:http");
  const requests = [];
  const server = createServer((request, response) => {
    let text = "";
    request.on("data", (chunk) => (text += chunk));
    request.on("end", () => {
      const body = JSON.parse(text);
      requests.push({ path: request.url, auth: request.headers.authorization, body });
      const answers = Object.fromEntries(
        Object.entries(body.questions).map(([id, question]) => {
          if (question.type === "noul") return [id, { type: "noul", noul: 0.9 }];
          const [first, ...rest] = Object.keys(question.criteria);
          const probabilities = {
            [first]: 0.8,
            ...Object.fromEntries(rest.map((o) => [o, 0.2 / rest.length])),
          };
          return [id, { type: "choice", choice: first, confidence: 0.7, probabilities }];
        }),
      );
      response.setHeader("content-type", "application/json");
      response.end(
        JSON.stringify({
          model: "jev-test",
          answers,
          usage: { input_tokens: 50, output_tokens: 3 },
        }),
      );
    });
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    requests,
    close: () => new Promise((done) => server.close(done)),
  };
}

const ASSESS_WORKFLOW = `
import { step, forEachFile } from "codemod/workflow";
import { assess, yesNo, choice } from "codemod/assess";
export default async function main() {
  const classified = await step("Classify", forEachFile("src/a.ts", async (file) => {
    const { answers, context, model } = await assess(file, {
      questions: { handlesErrors: yesNo("Does it handle errors?"), kind: choice(["library", "app"]) },
    });
    return { answers, context, model };
  }));
  const one = await step("One", async () => {
    const { answers } = await assess("src/b.ts", { questions: { ok: yesNo("Is it fine?") } });
    return answers.ok;
  });
  return { outputs: classified.outputs, assessed: classified.assessed, one };
}
`;

// The same tests on both hosts: Node (in this process) and the engine's
// QuickJS runtime (through the CLI).
for (const host of Object.keys(HOSTS)) {
  const runWorkflow = (workflow, options) => run(workflow, { ...options, host });

  test(
    `${host}: assess asks the decision model about a file and the files it imports`,
    HOSTS[host],
    async () => {
      const server = await fakeSystemOne();
      try {
        const dir = scratch({ "workflow.ts": ASSESS_WORKFLOW });
        const run = await runWorkflow(join(dir, "workflow.ts"), {
          target: {
            "src/a.ts": 'import { b } from "./b";\nexport const a = b;\n',
            "src/b.ts": "export const b = 1;\n",
          },
          decisionModel: { apiKey: "test-key", baseUrl: server.url },
        });
        assert.equal(run.status, "succeeded", run.error ?? undefined);
        assert.deepEqual(run.output, {
          outputs: [
            {
              answers: { handlesErrors: true, kind: "library" },
              context: ["src/a.ts", "src/b.ts"],
              model: "jev-test",
            },
          ],
          assessed: { assessments: 1, calls: 1, inputTokens: 50, outputTokens: 3 },
          one: true,
        });
        assert.equal(server.requests.length, 2);
        const [first] = server.requests;
        assert.equal(first.path, "/v1/systemone");
        assert.equal(first.auth, "Bearer test-key");
        assert.equal(first.body.model, "jev-latest");
        assert.match(first.body.state, /----- src\/b\.ts \(defines b\) -----\nexport const b = 1;/);
      } finally {
        await server.close();
      }
    },
  );

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

  test(
    `${host}: a reducer folds the outputs in path order, inline or imported`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "scripts/by-dir.ts": `
export default async function byDir(acc: Record<string, string[]>, output: number, path: string) {
  await Promise.resolve();
  const dir = path.split("/")[0];
  return { ...acc, [dir]: [...(acc[dir] ?? []), path + ":" + output] };
}
`,
          "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import byDir from "./scripts/by-dir.ts";
export default async function main() {
  const weight = 2;
  const inline = await step("Inline", forEachFile({ include: "**/*.ts", concurrency: 3 }, async (file) => {
    // Later files finish first. (Callbacks have no timers: they wait turns.)
    for (let turn = 0; turn < (file.path.includes("a") ? 200 : 1); turn++) await Promise.resolve();
    file.write(file.text.replace("var", "let"));
    return file.path.includes("skip") ? undefined : file.text.length;
  }, (acc, size, path) => ({ paths: [...acc.paths, path], total: acc.total + size * weight }), { paths: [], total: 0 }));
  const imported = await step("Imported", forEachFile("**/*.ts", (file) => file.text.length, byDir, {}));
  return { inline, imported: imported.reduced };
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, {
        target: {
          "x/a.ts": "var a = 1;\n",
          "x/b.ts": "var b = 1;\n",
          "y/c.ts": "var c = 1;\n",
          "y/skip.ts": "var s = 1;\n",
        },
      });
      assert.equal(result.status, "succeeded", result.error);
      assert.deepEqual(result.output.inline.reduced, {
        paths: ["x/a.ts", "x/b.ts", "y/c.ts"],
        total: 66,
      });
      assert.equal(result.output.inline.outputs, undefined);
      assert.deepEqual(result.output.inline.changes.written.length, 4);
      assert.deepEqual(result.output.imported, {
        x: ["x/a.ts:11", "x/b.ts:11"],
        y: ["y/c.ts:11", "y/skip.ts:11"],
      });
    },
  );

  test(
    `${host}: a reducer that throws fails the step and nothing is written`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Fold", forEachFile("*.ts", (file) => {
    file.write("changed\\n");
    return file.path;
  }, (count, path) => {
    if (path === "b.ts") throw new Error("cannot fold b");
    return count + 1;
  }, 0));
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, { target: { "a.ts": "a\n", "b.ts": "b\n" } });
      assert.equal(result.status, "failed");
      assert.match(result.error, /the reducer failed: .*cannot fold b/);
      assert.deepEqual(result.files, { "a.ts": "a\n", "b.ts": "b\n" });
    },
  );

  test(
    `${host}: a reducer that catches its own failStep() still fails the step`,
    HOSTS[host],
    async () => {
      const path = join(
        scratch({
          "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import { failStep } from "codemod/runtime";
export default async function main() {
  await step("Fold", forEachFile("*.ts", (file) => {
    file.write("changed\\n");
    return 1;
  }, (count, n) => {
    try { failStep("the fold is wrong"); } catch {}
    return count + n;
  }, 0));
}
`,
        }),
        "workflow.ts",
      );
      const result = await runWorkflow(path, { target: { "a.ts": "a\n" } });
      assert.equal(result.status, "failed");
      assert.match(result.error, /the reducer failed: .*the fold is wrong/);
      assert.deepEqual(result.files, { "a.ts": "a\n" });
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
import { step } from "codemod/workflow";
import { branch, commit } from "codemod/git";
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
      // Callbacks have no timers: busy for longer than the timeout.
      () => { const end = Date.now() + 400; while (Date.now() < end) {} },
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

test(
  "node: a dry run lets later steps see what earlier ones would change",
  HOSTS.node,
  async () => {
    const { runWorkflow: runOnHost } = await import("../runtime/host.mjs");
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile, astGrep, shard } from "codemod/workflow";
export default async function main() {
  await step("One", forEachFile("**/*.ts", (file) => {
    if (file.path === "gone.ts") return file.delete();
    file.write(file.text.replace("1", "2"));
    if (file.path === "old.ts") file.rename("new.ts");
  }));
  const seen = await step("Two", forEachFile("**/*.ts", (file) => {
    file.write(file.text.replace("2", "3"));
    return file.path;
  }));
  const rules = await step("Three", forEachFile(astGrep({ id: "let", language: "typescript", rule: { pattern: "const $A = 3" }, fix: "let $A = 3;" })));
  const shards = await step("Four", shard({ method: { type: "directory", maxFilesPerShard: 10 }, files: "**/*.ts", filter: (file) => file.text.includes("let") }));
  return { seen: seen.outputs, matches: rules.matches, sharded: shards.flatMap((s: any) => s._meta_files).sort() };
}
`,
      }),
      "workflow.ts",
    );
    const files = {
      "a.ts": "const a = 1;\n",
      "old.ts": "const o = 1;\n",
      "gone.ts": "const g = 1;\n",
    };
    const expected = { seen: ["a.ts", "new.ts"], matches: 2, sharded: ["a.ts", "new.ts"] };

    const real = scratch(files);
    const realRun = await runOnHost(path, { target: real });
    assert.equal(realRun.status, "succeeded", realRun.error);
    assert.deepEqual(realRun.output, expected);
    assert.equal(readFileSync(join(real, "new.ts"), "utf8"), "let o = 3;\n");

    const dry = scratch(files);
    const dryRun = await runOnHost(path, { target: dry, dryRun: true });
    assert.equal(dryRun.status, "succeeded", dryRun.error);
    assert.deepEqual(dryRun.output, expected);
    // Nothing is written.
    for (const [file, text] of Object.entries(files)) {
      assert.equal(readFileSync(join(dry, file), "utf8"), text);
    }
    assert.throws(() => readFileSync(join(dry, "new.ts")));
    // The run's diff is what the real run changed.
    const diffs = dryRun.events
      .filter((event) => event.event === "run_diff")
      .map((event) => event.path);
    assert.deepEqual(diffs, ["a.ts", "gone.ts", "new.ts", "old.ts"]);
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

test("node: workers stay within the reducer's window", HOSTS.node, async () => {
  const path = join(
    scratch({
      "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import { readFile } from "node:fs/promises";
export default async function main({ target }: { target: string }) {
  // On Node.js, callbacks and the reducer share globalThis.
  const { gap } = await step("Fold", forEachFile("*.txt", () => {
    const shared = globalThis as { started?: number };
    shared.started = (shared.started ?? 0) + 1;
    return 1;
  }, async (acc: { folded: number; gap: number }) => {
    // Waits a turn of the event loop, so workers get ahead. (Callbacks have
    // no timers; reading a file is allowed.)
    await readFile(target + "/f0000.txt");
    const started = (globalThis as { started?: number }).started ?? 0;
    return { folded: acc.folded + 1, gap: Math.max(acc.gap, started - acc.folded - 1) };
  }, { folded: 0, gap: 0 })).then((result) => result.reduced);
  return gap;
}
`,
    }),
    "workflow.ts",
  );
  const target = Object.fromEntries(
    Array.from({ length: 1400 }, (_, i) => [`f${String(i).padStart(4, "0")}.txt`, "x\n"]),
  );
  const result = await run(path, { target, host: "node" });
  assert.equal(result.status, "succeeded", result.error);
  assert.ok(result.output <= 1025, `workers got ${result.output} files ahead`);
});

// ------------------------------------------------------ pure callbacks ---

// What the CLI refuses workflow code, Node.js refuses too, so a test cannot
// pass here and fail with the CLI.

test("node: fetch in a function step fails the step, pointing at http()", HOSTS.node, async () => {
  const path = join(
    scratch({
      "workflow.ts": `
import { step } from "codemod/workflow";
export default async function main() {
  // Through a computed name: the build refuses \`fetch\` written out.
  const name = ["fe", "tch"].join("");
  await step("Fetch", async () => (await (globalThis as any)[name]("http://127.0.0.1:9/")).status);
}
`,
    }),
    "workflow.ts",
  );
  const result = await run(path, { target: {}, host: "node" });
  assert.equal(result.status, "failed");
  assert.match(result.error, /fetch is not available in workflow callbacks/);
  assert.match(result.error, /http\(url, options\)/);
  // Outside a run, the test itself still has it.
  assert.equal(typeof fetch, "function");
});

test(
  "node: child_process and fs writes in a forEachFile callback fail the file",
  HOSTS.node,
  async () => {
    const path = join(
      scratch({
        "workflow.ts": `
import { step, forEachFile } from "codemod/workflow";
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
export default async function main({ target }: { target: string }) {
  return await step("Files", forEachFile("*.ts", (file) => {
    // Reading stays allowed.
    readFileSync(target + "/" + file.path, "utf8");
    if (file.path === "exec.ts") execSync("echo hi");
    if (file.path === "write.ts") writeFileSync(target + "/side.txt", "x");
    file.write("changed\\n");
  }));
}
`,
      }),
      "workflow.ts",
    );
    const result = await run(path, {
      target: { "exec.ts": "a\n", "write.ts": "b\n", "ok.ts": "c\n" },
      host: "node",
    });
    assert.equal(result.status, "succeeded", result.error);
    const { failures } = result.step("Files").result;
    assert.equal(failures.length, 2, JSON.stringify(failures));
    assert.match(
      failures[0].error,
      /child_process\.execSync is not available in workflow callbacks/,
    );
    assert.match(failures[0].error, /shell\("command"\)/);
    assert.match(failures[1].error, /fs\.writeFileSync is not available in workflow callbacks/);
    assert.deepEqual(result.files, { "exec.ts": "a\n", "write.ts": "b\n", "ok.ts": "changed\n" });
  },
);

test("node: process.env can be read inside a run, not changed", HOSTS.node, async () => {
  const path = join(
    scratch({
      "workflow.ts": `
import { step } from "codemod/workflow";
export default async function main() {
  const path = await step("Read", () => typeof process.env.PATH);
  await step("Write", () => { process.env.CODEMOD_GUARD_TEST = "x"; });
  return path;
}
`,
    }),
    "workflow.ts",
  );
  const result = await run(path, { target: {}, host: "node" });
  assert.equal(result.status, "failed");
  assert.equal(result.step("Read").result, "string");
  assert.match(result.error, /process\.env cannot be changed in workflow callbacks/);
  assert.equal(process.env.CODEMOD_GUARD_TEST, undefined);
  // Outside a run it changes as ever.
  process.env.CODEMOD_GUARD_TEST = "y";
  assert.equal(process.env.CODEMOD_GUARD_TEST, "y");
  delete process.env.CODEMOD_GUARD_TEST;
});

test("node: timers fail in the body and in callbacks", HOSTS.node, async () => {
  const inBody = join(
    scratch({
      "workflow.ts": `
export default async function main() {
  // Through a computed name: the build refuses \`setTimeout\` written out.
  const wait = (globalThis as any)[["set", "Timeout"].join("")];
  await new Promise((done) => wait(done, 1));
}
`,
    }),
    "workflow.ts",
  );
  const body = await run(inBody, { target: {}, host: "node" });
  assert.equal(body.status, "failed");
  assert.match(body.error, /setTimeout is not available in the workflow body/);
  assert.match(body.error, /sleep\(/);

  const inCallback = join(
    scratch({
      "workflow.ts": `
import { step } from "codemod/workflow";
export default async function main() {
  const name = ["set", "Immediate"].join("");
  await step("Wait", () => new Promise((done) => (globalThis as any)[name](done)));
}
`,
    }),
    "workflow.ts",
  );
  const callback = await run(inCallback, { target: {}, host: "node" });
  assert.equal(callback.status, "failed");
  assert.match(callback.error, /setImmediate is not available in workflow callbacks/);
});

/** A local HTTP server answering JSON, counting its requests. */
async function countingServer() {
  const { createServer } = await import("node:http");
  const server = createServer((request, response) => {
    server.hits++;
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ hit: server.hits, method: request.method }));
  });
  server.hits = 0;
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  return { server, url: `http://127.0.0.1:${server.address().port}/data` };
}

test(
  "node: http() is recorded and not sent again on resume; the body's clock and random numbers replay",
  HOSTS.node,
  async () => {
    const { runWorkflow: runOnHost } = await import("../runtime/host.mjs");
    const { server, url } = await countingServer();
    const before = process.env.CODEMOD_RUN_STORE_DIR;
    process.env.CODEMOD_RUN_STORE_DIR = mkdtempSync(join(tmpdir(), "codemod-store-"));
    try {
      const path = join(
        scratch({
          "workflow.ts": `
import { step, http, approval } from "codemod/workflow";
export default async function main({ params }: { params: { url: string } }) {
  const start = Date.now();
  const random = Math.random();
  const uuid = crypto.randomUUID();
  const fetched = await step("Fetch", http(params.url));
  const after = Date.now();
  // Real time in a callback.
  const real = await step("Now", () => Date.now());
  console.log(JSON.stringify({ start, random, uuid, after }));
  await step("Go", approval("go on?"));
  return { start, after, real, fetched };
}
`,
        }),
        "workflow.ts",
      );
      const target = scratch({ "a.ts": "a\n" });
      const options = { target, params: { url }, store: "local", captureConsole: true };
      const seen = (report) =>
        report.events.filter((e) => e.event === "run_log").map((e) => JSON.parse(e.line));
      const started = Date.now();

      // Not approved as a whole, so the run stops at "Go"; its request is.
      const first = await runOnHost(path, { ...options, approve: false, approved: ["http:0"] });
      assert.equal(first.status, "awaiting-approval", first.error ?? undefined);
      assert.equal(server.hits, 1);
      const fetched = first.steps.find((s) => s.label === "Fetch");
      assert.equal(fetched.result.status, 200);
      assert.deepEqual(fetched.result.json, { hit: 1, method: "GET" });
      const [once] = seen(first);
      // The body's clock starts at the run's start time.
      assert.ok(once.start >= started - 1000 && once.start <= Date.now(), JSON.stringify(once));

      const second = await runOnHost(path, { ...options, resume: first.runId });
      assert.equal(second.status, "succeeded", second.error ?? undefined);
      // Recorded: not sent again.
      assert.equal(server.hits, 1);
      assert.deepEqual(second.output.fetched.json, { hit: 1, method: "GET" });
      // The same clock, numbers and ids on resume.
      assert.deepEqual(seen(second), [once]);
      // It moved to when the step finished; a callback has the real time.
      assert.ok(once.start <= once.after && once.after <= second.output.real);
      assert.match(
        once.uuid,
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
      );
    } finally {
      if (before === undefined) delete process.env.CODEMOD_RUN_STORE_DIR;
      else process.env.CODEMOD_RUN_STORE_DIR = before;
      server.close();
    }
  },
);

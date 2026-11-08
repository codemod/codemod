// Runs crates/codemod-sandbox/tests/integration/sg_api/api_report.js against
// the napi facade and requires the exact report the QuickJS facade produces
// (api_report.snapshot.json): same SgRoot/SgNode behaviour, same errors. Also
// covers the `JssgBatch` engine surface (the bridge's batch rules from
// crates/jssg-batch: planning, selector skip, result conversion).
//
//   pnpm --filter @codemod.com/jssg-napi build:debug && pnpm --filter @codemod.com/jssg-napi test
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { loadBinding, runTransform } from "../index.js";
import { importArtifact } from "../loader.js";
import { createHash } from "node:crypto";

const sgApi = resolve(
  import.meta.dirname,
  "../../../crates/codemod-sandbox/tests/integration/sg_api",
);
const INPUT = `import { a } from "./a";
const greeting = 'héllo';
console.log(a);
foo(1, "two", three);
console.log(bar);
`;

const sha256 = (text) => createHash("sha256").update(text).digest("hex");

/** A planned batch over `paths` in `root` (the JSON `jssg_batch::BatchConfig`). */
function batchFor(native, root, paths, extra = {}) {
  const source = extra.source ?? "export default () => null;";
  return new native.JssgBatch(
    JSON.stringify({
      transform: { name: extra.name ?? "t", hash: sha256(source) },
      source,
      language: "typescript",
      targetRoot: root,
      ...extra.config,
    }),
    paths,
  );
}

test("SgRoot/SgNode behave exactly like the QuickJS facade (sg_api snapshot)", async () => {
  const native = loadBinding();
  const temp = mkdtempSync(join(tmpdir(), "jssg-napi-"));
  try {
    const work = join(temp, "work");
    mkdirSync(work);
    writeFileSync(join(work, "input.ts"), INPUT);
    const root = realpathSync(work);
    const source = readFileSync(join(sgApi, "api_report.js"), "utf8");
    const transform = await importArtifact(
      { name: "api_report", hash: sha256(source), source },
      "test",
    );

    const batch = batchFor(native, root, ["input.ts"], {
      name: "api_report",
      source,
      config: { selector: { rule: { pattern: "console.log($A)" } } },
    });
    assert.equal(batch.module, "api_report.jssg.js");
    const [sgRoot] = await batch.parse(0, [INPUT]);
    assert.ok(sgRoot);
    const { result, staged } = await runTransform(root, {}, () =>
      transform(sgRoot, {
        ...batch.options(),
        matches: sgRoot.root().findAll({ rule: { pattern: "console.log($A)" } }),
      }),
    );
    let actual = result;
    for (const dir of [root, work]) actual = actual.replaceAll(dir, "<WORK>");
    const expected = readFileSync(join(sgApi, "api_report.snapshot.json"), "utf8");
    assert.deepEqual(JSON.parse(actual), JSON.parse(expected));
    assert.equal(actual, expected);
    // The report renames the file: the outcome keeps the content at the new path.
    assert.deepEqual(batch.finish(0, sgRoot, result, staged), {
      path: "input.ts",
      edits: [{ path: "input.ts", content: result, renameTo: "renamed.ts" }],
    });
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test("a selector that matches nothing skips the file before JavaScript sees it", async () => {
  const native = loadBinding();
  const temp = realpathSync(mkdtempSync(join(tmpdir(), "jssg-napi-")));
  try {
    writeFileSync(join(temp, "a.ts"), "oldApi(1);");
    writeFileSync(join(temp, "b.ts"), "other(1);");
    const batch = batchFor(native, temp, ["a.ts", "b.ts"], {
      config: { selector: { rule: { pattern: "oldApi($A)" } }, input: { n: 1 } },
    });
    const roots = await batch.parse(0, ["oldApi(1);", "other(1);"]);
    assert.equal(roots[1], null);
    assert.equal(roots[0].relativeFilename(), "a.ts");
    assert.equal(roots[0].filename(), join(temp, "a.ts"));
    assert.deepEqual(batch.options(), {
      params: { input: { n: 1 } },
      language: "TypeScript",
      matches: undefined,
      matrixValues: undefined,
      dryRun: false,
      targetDir: temp,
    });
    assert.throws(
      () => batchFor(native, temp, ["a.ts"], { config: { selector: { rule: 5 } } }),
      /invalid JSSG selector/u,
    );
    assert.throws(
      () => batchFor(native, temp, ["a.ts"], { config: { language: "klingon" } }),
      /invalid JSSG language 'klingon'/u,
    );
    assert.throws(() => batchFor(native, temp, ["../x.ts"]), /must be a safe relative path/u);
    assert.equal(
      batchFor(native, temp, ["a.ts"], { config: { semanticAnalysis: "file" } }).indexesWorkspace,
      false,
    );
    assert.equal(
      batchFor(native, temp, ["a.ts"], { config: { semanticAnalysis: "workspace" } })
        .indexesWorkspace,
      true,
    );
    assert.throws(
      () =>
        batchFor(native, temp, ["a.ts"], {
          config: { semanticAnalysis: { mode: "file", root: "src" } },
        }),
      /semanticAnalysis.root requires workspace mode/u,
    );
    assert.throws(() => native.engineLanguageName("klingon"), /invalid JSSG language 'klingon'/u);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test("results convert with the sandbox's rules", async () => {
  const native = loadBinding();
  const temp = realpathSync(mkdtempSync(join(tmpdir(), "jssg-napi-")));
  try {
    writeFileSync(join(temp, "a.ts"), "a;");
    const batch = batchFor(native, temp, ["a.ts"]);
    const finish = async (value) => {
      const [root] = await batch.parse(0, ["a;"]);
      const { result, staged } = await runTransform(temp, {}, () => value);
      return batch.finish(0, root, result, staged);
    };
    assert.deepEqual(await finish("b;"), {
      path: "a.ts",
      edits: [{ path: "a.ts", content: "b;" }],
    });
    // No edits and no output: `null` (the engine builds `{ path, edits: [] }`).
    for (const unchanged of ["a;", null, undefined]) {
      assert.equal(await finish(unchanged), null, String(unchanged));
    }
    assert.deepEqual(
      await finish({ content: null, output: { a: undefined, b: [1, 1.5, () => 1], c: 2 ** 40 } }),
      { path: "a.ts", edits: [], output: { a: null, b: [1, 1.5, null], c: 2 ** 40 } },
    );
    for (const [value, message] of [
      [42, /JSSG failed for 'a\.ts': .*Received int$/u],
      [1.5, /Received float$/u],
      [true, /Received bool$/u],
      [{ content: "x" }, /must contain an 'output' field/u],
      [[1], /must contain an 'output' field/u],
    ]) {
      await assert.rejects(finish(value), message);
    }
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

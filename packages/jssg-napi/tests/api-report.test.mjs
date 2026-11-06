// Runs crates/codemod-sandbox/tests/integration/sg_api/api_report.js against
// the napi facade and requires the exact report the QuickJS facade produces
// (api_report.snapshot.json): same SgRoot/SgNode behaviour, same errors.
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

test("SgRoot/SgNode behave exactly like the QuickJS facade (sg_api snapshot)", async () => {
  const native = loadBinding();
  const temp = mkdtempSync(join(tmpdir(), "jssg-napi-"));
  try {
    const work = join(temp, "work");
    mkdirSync(work);
    writeFileSync(join(work, "input.ts"), INPUT);
    const root = realpathSync(work);
    const source = readFileSync(join(sgApi, "api_report.js"), "utf8");
    const hash = createHash("sha256").update(source).digest("hex");
    const transform = await importArtifact({ name: "api_report", hash, source }, "test");

    const selector = new native.Selector("typescript", { rule: { pattern: "console.log($A)" } });
    const [sgRoot] = await native.engineParseBatch(
      "typescript",
      root,
      ["input.ts"],
      [INPUT],
      selector,
    );
    assert.ok(sgRoot);
    const { result, staged } = await runTransform(root, {}, () =>
      transform(sgRoot, {
        params: {},
        language: native.engineLanguageName("typescript"),
        matches: sgRoot.root().findAll({ rule: { pattern: "console.log($A)" } }),
        matrixValues: undefined,
        dryRun: false,
        targetDir: root,
      }),
    );
    assert.deepEqual(staged, []);
    assert.match(native.engineRenameTo(sgRoot), /work\/renamed\.ts$/u);
    let actual = result;
    for (const dir of [root, work]) actual = actual.replaceAll(dir, "<WORK>");
    const expected = readFileSync(join(sgApi, "api_report.snapshot.json"), "utf8");
    assert.deepEqual(JSON.parse(actual), JSON.parse(expected));
    assert.equal(actual, expected);
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
    const selector = new native.Selector("typescript", { rule: { pattern: "oldApi($A)" } });
    const roots = await native.engineParseBatch(
      "typescript",
      temp,
      ["a.ts", "b.ts"],
      ["oldApi(1);", "other(1);"],
      selector,
    );
    assert.equal(roots[1], null);
    assert.equal(roots[0].relativeFilename(), "a.ts");
    assert.equal(roots[0].filename(), join(temp, "a.ts"));
    assert.throws(() => new native.Selector("typescript", { rule: 5 }), /invalid JSSG selector/u);
    assert.throws(() => native.engineLanguageName("klingon"), /invalid JSSG language 'klingon'/u);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

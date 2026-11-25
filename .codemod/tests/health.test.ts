import assert from "node:assert/strict";
import { test } from "node:test";
import { runWorkflow } from "codemod/testing";
import type { Report } from "../workflows/lib/report.ts";

const workflow = new URL("../workflows/health.ts", import.meta.url);
const fixture = (name: string) => new URL(`./fixtures/${name}/`, import.meta.url);

/** Runs health.ts on a fixture with the CLI's runtime (`codemod/deps` isn't on the Node host yet). */
async function health(name: string) {
  const run = await runWorkflow(workflow, { target: fixture(name), host: "quickjs" });
  assert.deepEqual(run.changes, {}, "a check never writes");
  // The report is the run's output, also when a check failed the run.
  const report = run.output as Report | null;
  assert.ok(report, `no output; the run ${run.status}: ${run.error}`);
  return { run, report };
}

const check = (report: Report, id: string) => report.checks.find((c) => c.id === id)!;

test("pass: one crossterm, only allowed @latest mentions", async () => {
  const { run, report } = await health("pass");
  assert.equal(run.status, "succeeded");
  assert.deepEqual(
    report.checks.map((c) => [c.id, c.status]),
    [
      ["single-version-crates", "pass"],
      ["latest-pins", "pass"],
    ],
  );
  assert.deepEqual(check(report, "single-version-crates").metrics, { crossterm_versions: 1 });
});

test("duplicate-crate: the extra crossterm and who pulls it in", async () => {
  const { run, report } = await health("duplicate-crate");
  assert.equal(run.status, "failed");
  assert.equal(run.error, "health: 1 check(s) failed: single-version-crates (1 findings)");
  const crates = check(report, "single-version-crates");
  assert.equal(crates.status, "fail");
  assert.deepEqual(crates.metrics, { crossterm_versions: 2 });
  assert.deepEqual(crates.findings, [
    {
      message:
        "crossterm@0.28.1 next to crossterm@0.29.0: codemod > ratatui > crossterm (pulled in by ratatui@0.29.0)",
      file: "Cargo.lock",
    },
  ]);
  assert.equal(check(report, "latest-pins").status, "pass");
});

test("latest-pin: every pin with file, line and the command", async () => {
  const { run, report } = await health("latest-pin");
  assert.equal(run.status, "failed");
  const pins = check(report, "latest-pins");
  assert.equal(pins.status, "fail");
  assert.deepEqual(pins.metrics, { pins: 3, files: 2 });
  assert.deepEqual(
    pins.findings!.map((f) => `${f.file}:${f.line} ${f.message.split("`")[1]}`),
    [
      "crates/cli/src/hints.rs:2 codemod@latest",
      "docs/guides/ci.mdx:4 npm install -g codemod@latest",
      "docs/guides/ci.mdx:5 npx codemod@latest",
    ],
  );
});

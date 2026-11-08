/**
 * Benchmark timings end to end: `codemod-workflow run` on every runtime with
 * `CODEMOD_TIMINGS_FILE` set writes one top-level file (`ts-<runtime>`) plus
 * one file per child process (the quickjs host), whose merged
 * phases cover the run without double counting; unset, nothing is written.
 *
 * Needs binaries and the addon built with the `timings` feature:
 *   pnpm --filter @codemod.com/orchestration test:e2e:timings
 * (`CODEMOD_QUICKJS_HOST_BIN` / `CODEMOD_JSSG_NAPI`
 * override the debug builds, e.g. with the release ones the benchmark uses).
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";

const enabled = process.env.CODEMOD_TIMINGS_E2E === "1";
const repoRoot = resolve(import.meta.dirname, "../../..");
const cli = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");
const transforms = join(repoRoot, "benchmarks/runtimes/transforms");

interface Phase {
  ns: number;
  count: number;
  cpu_ns?: number;
}
interface TimingsFile {
  schema: number;
  arm: string;
  pid: number;
  wall_ns: number;
  phases: Record<string, Phase>;
  children: string[];
}

const FILES = 3;
let workflow: string;

beforeAll(() => {
  if (!enabled) return;
  const dir = mkdtempSync(join(tmpdir(), "codemod-timings-wf-"));
  workflow = join(dir, "workflow.ts");
  writeFileSync(
    workflow,
    `import { jssg, sequence } from ${JSON.stringify(resolve(import.meta.dirname, "../src/index.ts"))};
import consoleToLogger from ${JSON.stringify(join(transforms, "console-to-logger.ts"))};
import renameVitestImport from ${JSON.stringify(join(transforms, "rename-vitest-import.ts"))};
const files = { language: "typescript", include: ["**/*.ts"] } as const;
const a = jssg({ name: "console-to-logger", ...files, transform: consoleToLogger });
const b = jssg({ name: "rename-vitest-import", ...files, transform: renameVitestImport });
export default sequence(a(), b());
`,
  );
});

function makeRepo(): string {
  const repo = mkdtempSync(join(tmpdir(), "codemod-timings-repo-"));
  mkdirSync(join(repo, "src"));
  for (let i = 0; i < FILES; i++) {
    writeFileSync(
      join(repo, "src", `f${i}.ts`),
      `import { it } from "vitest";\nconsole.log("f${i}", ${i});\n`,
    );
  }
  return repo;
}

function runCli(runtime: string, repo: string, timings: string | undefined, cwd?: string) {
  const env: Record<string, string | undefined> = { ...process.env };
  delete env.CODEMOD_TIMINGS_FILE;
  delete env.CODEMOD_TIMINGS_ROOT_PID;
  delete env.BENCH_T0_NS;
  if (timings !== undefined) env.CODEMOD_TIMINGS_FILE = timings;
  const result = spawnSync(
    process.execPath,
    [cli, "run", `--runtime=${runtime}`, workflow, "--target", repo],
    { env, encoding: "utf8", ...(cwd === undefined ? {} : { cwd }) },
  );
  expect(result.status, result.stderr).toBe(0);
  expect(readFileSync(join(repo, "src/f0.ts"), "utf8")).toContain('logger.info("f0", 0)');
}

const read = (path: string) => JSON.parse(readFileSync(path, "utf8")) as TimingsFile;
const total = (phases: Record<string, Phase>) =>
  Object.values(phases).reduce((sum, phase) => sum + phase.ns, 0);

describe.runIf(enabled)("benchmark timings files", () => {
  const children = { node: [], quickjs: ["quickjs-host"] };

  for (const [runtime, childArms] of Object.entries(children)) {
    it(`${runtime}: one top-level file, one per child, phases within the wall`, () => {
      const repo = makeRepo();
      const dir = mkdtempSync(join(tmpdir(), "codemod-timings-out-"));
      const path = join(dir, "run.timings.json");
      runCli(runtime, repo, path);

      const top = read(path);
      expect(top.schema).toBe(1);
      expect(top.arm).toBe(`ts-${runtime}`);
      expect(top.pid).toBeGreaterThan(0);
      // The CLI's own phases; the rest may come from a child (quickjs host).
      for (const name of ["boot", "plan"]) {
        expect(top.phases[name]?.count ?? 0, `${runtime} ${name}`).toBeGreaterThan(0);
      }
      const docs = top.children.map(read);
      expect(docs.map((doc) => doc.arm)).toEqual(childArms);
      // Every child file on disk is listed.
      expect(readdirSync(dir).length).toBe(1 + childArms.length);

      const merged: Record<string, Phase> = {};
      for (const doc of [top, ...docs]) {
        for (const [name, phase] of Object.entries(doc.phases)) {
          const into = (merged[name] ??= { ns: 0, count: 0 });
          into.ns += phase.ns;
          into.count += phase.count;
        }
      }
      for (const name of ["select", "read", "setup", "parse", "conflict", "commit"]) {
        expect(merged[name]?.count ?? 0, `${runtime} ${name}`).toBeGreaterThan(0);
      }
      // Two transforms over every file, one call each.
      expect(merged.transform?.count).toBe(2 * FILES);
      // Merged phases never exceed the top-level wall (no double counting)
      // and cover most of it even on a tiny repository.
      const covered = total(merged) / top.wall_ns;
      expect(covered).toBeLessThanOrEqual(1.01);
      expect(covered).toBeGreaterThan(0.8);
    });
  }

  it("writes nothing without CODEMOD_TIMINGS_FILE", () => {
    const cwd = mkdtempSync(join(tmpdir(), "codemod-timings-off-"));
    for (const runtime of Object.keys(children)) {
      const repo = makeRepo();
      runCli(runtime, repo, undefined, cwd);
      expect(readdirSync(join(repo, "src")).sort()).toEqual(
        Array.from({ length: FILES }, (_, i) => `f${i}.ts`).sort(),
      );
    }
    expect(readdirSync(cwd)).toEqual([]);
  });
});

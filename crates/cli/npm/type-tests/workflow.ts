// Type tests for the workflow.ts API in `codemod/workflow`. Checked with
// `pnpm --filter codemod typecheck`; nothing here runs.

import type TS from "codemod/ast-grep/langs/typescript";
import { parse } from "codemod/ast-grep";
import type { Codemod } from "codemod/ast-grep";
import {
  agent,
  approval,
  astGrep,
  branch,
  codemod,
  commit,
  forEachFile,
  installSkill,
  pullRequest,
  shard,
  shell,
  sleep,
  step,
  waitForEvent,
  type ShardResult,
  type WorkflowContext,
} from "codemod/workflow";
import migrate from "./scripts/migrate.ts";
import sumByPackage from "./scripts/sum-by-package.ts";
import shardFiles from "./scripts/shard.ts";
import wouldChange from "./scripts/would-change.ts";
import rules from "./rules/config.yml";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
function expectType<T extends true>(_: T) {}

export default async function main({
  params,
  target,
  dryRun,
}: WorkflowContext<{ replacement?: string }>) {
  expectType<Equal<typeof params.replacement, string | undefined>>(true);
  expectType<Equal<typeof target, string>>(true);
  expectType<Equal<typeof dryRun, boolean>>(true);

  // ── forEachFile ──────────────────────────────────────────────────────

  const found = await step(
    "Find",
    forEachFile({ language: "typescript", containing: "moment" }, (file) => {
      const root = parse<TS>(file);
      const imports = root.root().findAll({ rule: { pattern: 'import $X from "moment"' } });
      return imports.length ? file.path : undefined;
    }),
  );
  expectType<Equal<typeof found.outputs, readonly string[]>>(true);
  expectType<Equal<(typeof found.changes.renamed)[number]["to"], string>>(true);

  // Returning nothing collects nothing.
  const renamed = await step(
    forEachFile("src/**/*.less", (file) => {
      file.rename(file.path.replace(/\.less$/, ".css"));
    }),
  );
  expectType<Equal<typeof renamed.outputs, readonly never[]>>(true);

  // Selectors: a glob, globs, an object, or none (the package's languages).
  forEachFile(["**/*.ts", "**/*.tsx"], () => undefined);
  forEachFile({ include: "**/*.vue", language: "html" }, () => undefined);
  forEachFile({ exclude: "dist/**", concurrency: 4 }, () => undefined);
  forEachFile((file) => file.path);
  // @ts-expect-error a language without include must be one forEachFile can select files by
  forEachFile({ language: "kotlin" }, () => undefined);

  // Callbacks may be async; outputs are what they resolve to.
  const lengths = await step(
    forEachFile("**/*.ts", async (file) => {
      await Promise.resolve();
      return { path: file.path, size: file.text.length };
    }),
  );
  expectType<Equal<(typeof lengths.outputs)[number]["size"], number>>(true);

  // parse takes the file, text, or (lang, text).
  forEachFile({ language: "tsx" }, (file) => {
    const fromFile = parse<TS>(file);
    const fromText = parse("let a = 1;");
    const withLanguage = parse(file.text, { language: "tsx" });
    const legacy = parse("tsx", file.text);
    void fromFile;
    expectType<Equal<typeof file.language, string | undefined>>(true);
    return [fromText, withLanguage, legacy].length;
  });

  // The error names what is not JSON (`NotJson<"a Set: use an array">`),
  // not astGrep's handler, which forEachFile also takes.
  // @ts-expect-error a Set is not JSON
  forEachFile("**/*.ts", (file) => new Set([file.path]));
  // @ts-expect-error a Set is not JSON, without a selector too
  forEachFile((file) => new Set([file.path]));
  // @ts-expect-error what an async callback resolves to must be JSON: a Map is not
  forEachFile({ language: "tsx" }, async (file) => new Map([[file.path, 1]]));
  // @ts-expect-error a nested function is not JSON
  forEachFile("**/*.ts", (file) => ({ path: file.path, run: () => 1 }));

  forEachFile("**/*.ts", (file) => {
    // @ts-expect-error file.text is read-only; use file.write()
    file.text = "";
  });

  // ── function steps ───────────────────────────────────────────────────

  const summary = await step("Summarize", () => ({ files: found.outputs.length, names: ["a"] }));
  expectType<Equal<typeof summary, { readonly files: number; readonly names: readonly string[] }>>(
    true,
  );
  // @ts-expect-error results are frozen
  summary.files = 2;

  const nothing = await step(() => {});
  expectType<Equal<typeof nothing, null>>(true);

  // ── how a step is run, and waiting ───────────────────────────────────

  await step("Install", shell("pnpm install"), { retries: 2, retryDelay: "5s", timeout: "10m" });
  await step(() => 1, { timeout: 500 });
  // @ts-expect-error not a duration
  await step(() => 1, { timeout: "soon" });
  // @ts-expect-error unknown option
  await step(() => 1, { retry: 1 });

  expectType<Equal<Awaited<ReturnType<typeof sleep>>, null>>(true);
  await sleep("30s");
  // @ts-expect-error not a duration
  await sleep("a while");

  const ci = await waitForEvent<{ passed: boolean }>("ci-finished", { timeout: "1h" });
  expectType<Equal<typeof ci, { readonly passed: boolean }>>(true);

  const fromAsync = await step(async () => 42);
  expectType<Equal<typeof fromAsync, number>>(true);

  // Interfaces work as long as their members are JSON.
  interface Plan {
    batches: string[][];
  }
  const plan: Plan = { batches: [] };
  await step(() => plan);

  // @ts-expect-error a Map is not JSON
  await step(() => new Map<string, number>());
  // @ts-expect-error a Date is not JSON
  await step(() => ({ at: new Date() }));

  // ── tasks ────────────────────────────────────────────────────────────

  // An imported callback: what it returns is the step's outputs.
  const migrated = await step(forEachFile({ language: "typescript" }, migrate));
  expectType<
    Equal<typeof migrated.outputs, readonly { readonly path: string; readonly count: number }[]>
  >(true);
  await step(
    forEachFile(
      {
        include: "src/**/*.tsx",
        exclude: "**/*.test.tsx",
        language: "tsx",
        semanticAnalysis: "workspace",
        capabilities: ["fs"],
      },
      migrate,
    ),
  );
  await step(
    forEachFile({ language: "tsx", semanticAnalysis: { mode: "workspace", root: "src" } }, migrate),
  );
  // @ts-expect-error unknown capability
  forEachFile({ language: "tsx", capabilities: ["network"] }, migrate);
  // @ts-expect-error a callback is imported or written, not named by path
  forEachFile({ language: "tsx" }, "./scripts/migrate.ts");
  const ruled = await step(forEachFile(["src/**/*.ts"], astGrep(rules)));
  expectType<Equal<typeof ruled.matches, number>>(true);
  await step(forEachFile(astGrep(rules)));
  await step(
    forEachFile(
      { include: "src/**", exclude: "**/*.test.ts", containing: "moment" },
      astGrep(rules),
    ),
  );
  // @ts-expect-error the rules name their languages
  forEachFile({ language: "tsx" }, astGrep(rules));
  // @ts-expect-error semantic analysis is for callbacks
  forEachFile({ semanticAnalysis: "workspace" }, astGrep(rules));
  // @ts-expect-error astGrep is not a step on its own
  void step(astGrep(rules));
  // @ts-expect-error the selector says which files
  astGrep(rules, { include: ["**/*.ts"] });
  // Repeat until nothing changes: up to 10 passes, or as many as given.
  const repeated = await step(forEachFile(astGrep(rules, { repeat: true })));
  expectType<Equal<typeof repeated.matches, number>>(true);
  await step(forEachFile("src/**", astGrep(rules, { repeat: 3 })));
  // @ts-expect-error repeat is a boolean or a number of passes
  astGrep(rules, { repeat: "yes" });
  // @ts-expect-error only callbacks and astGrep
  forEachFile(shell("echo hi"));

  const ran = await step(shell("echo hi", { cwd: target, env: { A: "1" } }));
  expectType<Equal<typeof ran.stdout, string | undefined>>(true);
  if (!ran.skipped) expectType<Equal<typeof ran.exitCode, number>>(true);
  await step(shell("exit 3", { allowFailure: true }));
  const tagged = await step(shell`${target}/scripts/setup.sh ${["a b", 1, true]}`);
  expectType<Equal<typeof tagged, typeof ran>>(true);
  await step(shell({ cwd: target })`npm run build`);
  // @ts-expect-error a template takes strings, numbers and booleans
  void step(shell`echo ${{ not: "a word" }}`);

  // Rules written in the workflow, or built from values it has.
  await step(
    forEachFile(
      astGrep([
        {
          id: "no-var",
          language: "javascript",
          rule: { pattern: "var $A = $B" },
          fix: "let $A = $B;",
        },
        {
          id: "rename",
          language: "typescript",
          rule: {
            kind: "call_expression",
            has: { field: "function", regex: `^${target}$`, stopBy: "end" },
          },
          fix: { template: "renamed()", expandEnd: { regex: ";", stopBy: "end" } },
          constraints: { A: { not: { kind: "identifier" } } },
          transform: { B: { convert: { source: "$A", toCase: "camelCase" } } },
          severity: "warning",
        },
      ]),
    ),
  );
  // @ts-expect-error rules are imported or written, not named by path
  astGrep("./rules/config.yml");
  // @ts-expect-error a rule needs a language
  astGrep({ id: "x", rule: { pattern: "x" } });

  const shards = await step(
    shard({
      method: { type: "directory", maxFilesPerShard: 20 },
      target: "./src",
      files: { include: ["**/*.ts"] },
      filter: wouldChange,
    }),
  );
  await Promise.all(
    shards.map((current) =>
      step(`Migrate ${current.name}`, forEachFile({ include: current._meta_files }, migrate)),
    ),
  );
  expectType<Equal<(typeof shards)[number], Frozen<ShardResult>>>(true);
  // @ts-expect-error maxFilesPerShard is required
  shard({ method: { type: "directory" } });
  await step(shard({ method: { function: shardFiles }, files: "**/*.ts" }));
  await step(
    shard({
      method: { function: ({ files }) => [{ name: "all", _meta_shard: 0, _meta_files: files }] },
      filter: (file) => file.text.includes("moment"),
    }),
  );
  // @ts-expect-error shard functions are imported or written, not named by path
  shard({ method: { function: "./scripts/shard.ts" } });
  // @ts-expect-error shard's jssg was removed
  shard({ method: { function: shardFiles }, jssg: { path: "./scripts/codemod.ts" } });

  await step(approval("Ship it?"));
  await step(codemod("@codemod/jest-to-vitest", { params: {} }));
  const handed = await step(agent("Fix the tests", { maxSteps: 5 }));
  expectType<Equal<typeof handed.handedOff, boolean | undefined>>(true);
  await step(installSkill({ package: "@codemod/jest-to-vitest", harness: "claude" }));
  await step(branch("codemod/migrate"));
  await step(commit("refactor: migrate", { add: ["src"] }));
  const pr = await step(pullRequest({ title: "Migrate", draft: true }));
  expectType<Equal<typeof pr.url, string | null | undefined>>(true);

  // ── values between steps ─────────────────────────────────────────────

  // A shell step's output is parsed by the workflow; a later step gets it as params.
  const detected = await step(shell("./detect.sh"));
  const detectedParams: { strict?: boolean } = JSON.parse(detected.stdout ?? "{}");
  await step(
    forEachFile({ language: "tsx" }, (file) => (detectedParams.strict ? file.path : undefined)),
  );
  // @ts-expect-error steps take no state
  void step(shell("true"), { state: { target: "react-19" } });

  return { files: found.outputs.length };
}

import type { Frozen } from "codemod/workflow";

// A transform returns its edit and any JSON as data.
export const counts: Codemod<TS> = async (root) => ({
  edit: null,
  data: { calls: [{ file: root.filename(), deep: true, depth: 2 }] },
});
export const aNumber: Codemod<TS> = async () => ({ data: 1 });
// @ts-expect-error data is JSON
export const notJson: Codemod<TS> = async () => ({ data: () => 1 });

// ── forEachFile with a reducer ─────────────────────────────────────────

export async function reducers() {
  // The outputs are folded into `reduced`, typed by the initial value and
  // what the reducer returns; there are no `outputs`.
  const counted = await step(
    forEachFile(
      "**/*.ts",
      (file) => (file.text.includes("var") ? { path: file.path, count: 1 } : undefined),
      (total, output, path) => {
        expectType<Equal<typeof output, { path: string; count: number }>>(true);
        expectType<Equal<typeof path, string>>(true);
        return total + output.count;
      },
      0,
    ),
  );
  expectType<Equal<typeof counted.reduced, number>>(true);
  expectType<Equal<(typeof counted.failures)[number]["path"], string>>(true);
  // @ts-expect-error a reducer's result has no outputs
  void counted.outputs;
  const grouped = await step(
    forEachFile(
      { language: "typescript" },
      async (file) => file.text.length,
      async (sizes, size, path) => ({ ...sizes, [path]: size }),
      {} as Record<string, number>,
    ),
  );
  expectType<Equal<typeof grouped.reduced, { readonly [x: string]: number }>>(true);
  // An imported reducer, typed with FileReducer.
  const none: Record<string, number> = {};
  const byPackage = await step(
    forEachFile("**/*.ts", (file) => file.text.length, sumByPackage, none),
  );
  expectType<Equal<typeof byPackage.reduced, { readonly [x: string]: number }>>(true);
  const paths = (file: { path: string }) => file.path;
  // @ts-expect-error the reducer's result must be JSON: a Set is not
  forEachFile("**/*.ts", paths, (all, path) => all.add(path), new Set<string>());
  // @ts-expect-error the reducer returns what the initial value is
  forEachFile("**/*.ts", paths, (count: number) => String(count), 0);
  // @ts-expect-error astGrep(...) returns nothing per file: there is nothing to reduce
  forEachFile("**/*.ts", astGrep(rules), (count: number) => count + 1, 0);
}

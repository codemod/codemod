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
  jssg,
  pullRequest,
  shard,
  shell,
  sleep,
  step,
  waitForEvent,
  type ShardResult,
  type WorkflowContext,
} from "codemod/workflow";

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

  // @ts-expect-error a Set is not JSON
  forEachFile("**/*.ts", (file) => new Set([file.path]));
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

  const transformed = await step(
    jssg("./scripts/codemod.ts", {
      include: "src/**/*.ts",
      params: { library: params.replacement ?? "dayjs" },
      capabilities: ["fs"],
    }),
  );
  expectType<Equal<typeof transformed.unmodified, number>>(true);
  // @ts-expect-error unknown capability
  jssg("./scripts/codemod.ts", { capabilities: ["network"] });

  // A transform is run on the files `forEachFile` selects.
  const migrated = await step(forEachFile(jssg("./scripts/codemod.ts")));
  expectType<Equal<(typeof migrated.data)[number]["file"], string>>(true);
  // A reducer's result is the step's data.
  const counted = await step(
    jssg("./scripts/codemod.ts", {
      reduce: (total, { data }) => total + (typeof data === "number" ? data : 0),
      initial: 0,
    }),
  );
  expectType<Equal<typeof counted.data, number>>(true);
  // @ts-expect-error a reducer needs an initial value
  jssg("./scripts/codemod.ts", { reduce: (total: number) => total });
  expectType<Equal<typeof migrated.unmodified, number>>(true);
  await step(
    forEachFile(
      { include: "src/**/*.tsx", exclude: "**/*.test.tsx", language: "tsx" },
      jssg("./scripts/codemod.ts", { params: { strict: true } }),
    ),
  );
  const ruled = await step(forEachFile(["src/**/*.ts"], astGrep("rules/config.yml")));
  expectType<Equal<typeof ruled.matches, number>>(true);
  await step(forEachFile(astGrep("rules/config.yml")));
  // @ts-expect-error `containing` is for callbacks
  forEachFile({ containing: "moment" }, jssg("./scripts/codemod.ts"));
  // @ts-expect-error the rules name their languages
  forEachFile({ language: "tsx" }, astGrep("rules/config.yml"));
  // @ts-expect-error only jssg and astGrep are transforms
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

  await step(astGrep("rules/config.yml", { include: ["**/*.ts"] }));

  const shards = await step(
    shard({
      method: { type: "directory", maxFilesPerShard: 20 },
      target: "./src",
      jssg: { path: "scripts/codemod.ts", include: ["**/*.ts"] },
    }),
  );
  await Promise.all(
    shards.map((current) =>
      step(`Migrate ${current.name}`, jssg("scripts/codemod.ts", { files: current._meta_files })),
    ),
  );
  expectType<Equal<(typeof shards)[number], Frozen<ShardResult>>>(true);
  // @ts-expect-error maxFilesPerShard is required
  shard({ method: { type: "directory" } });

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
  await step(jssg("scripts/codemod.ts", { params: JSON.parse(detected.stdout ?? "{}") }));
  // @ts-expect-error steps take no state
  void step(shell("true"), { state: { target: "react-19" } });

  return { files: found.outputs.length };
}

import type { Frozen, JsonValue } from "codemod/workflow";

// A transform returns its edit and any JSON as data.
export const counts: Codemod<TS> = async (root) => ({
  edit: null,
  data: { calls: [{ file: root.filename(), deep: true, depth: 2 }] },
});
export const aNumber: Codemod<TS> = async () => ({ data: 1 });
// @ts-expect-error data is JSON
export const notJson: Codemod<TS> = async () => ({ data: () => 1 });

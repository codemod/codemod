// Type tests for `codemod/assess`. Checked with `pnpm --filter codemod typecheck`;
// nothing here runs. The workflow is the example in the reference docs.

import { parse } from "codemod/ast-grep";
import { assess, choice, list, number, yesNo } from "codemod/assess";
import { forEachFile, step } from "codemod/workflow";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
function expectType<T extends true>(_: T) {}

// Questions are data: define them once, at module level.
const QUESTIONS = {
  handlesErrors: yesNo("Does this file handle the errors its async calls can throw?"),
  smell: choice("Which error-handling smell fits it best?", {
    swallowed: "Errors are caught and dropped",
    "logged-only": "Errors are caught and only logged",
    none: null,
  }),
  risk: number("How risky is it to change this file's error handling?", { min: 1, max: 5 }),
  uses: list("Which of these does it use?", ["fetch", "fs", "database"]),
};

export default async function main() {
  // 1. Classify each file, with the files it imports as context.
  const classified = await step(
    "Classify error handling",
    forEachFile({ language: "typescript", concurrency: 4 }, async (file) => {
      const { answers, details, context, usage } = await assess(file, {
        questions: QUESTIONS,
        context: { imports: true, dependents: 3 },
        allowMoreContext: 1,
        timeout: "30s",
      });
      expectType<Equal<typeof answers.handlesErrors, boolean>>(true);
      expectType<Equal<typeof answers.smell, "swallowed" | "logged-only" | "none">>(true);
      expectType<Equal<typeof answers.risk, number>>(true);
      expectType<Equal<typeof answers.uses, ("fetch" | "fs" | "database")[]>>(true);
      expectType<
        Equal<
          typeof details.smell.probabilities,
          Record<"swallowed" | "logged-only" | "none", number>
        >
      >(true);
      expectType<Equal<typeof details.handlesErrors, { probability: number }>>(true);
      expectType<Equal<typeof context, string[]>>(true);
      expectType<Equal<typeof usage.calls, number>>(true);
      // @ts-expect-error not a question
      void answers.missing;
      return { path: file.path, smell: answers.smell, sure: details.smell.confidence > 0.8 };
    }),
  );
  expectType<
    Equal<(typeof classified.outputs)[number]["smell"], "swallowed" | "logged-only" | "none">
  >(true);
  // What the step's assess() calls used.
  expectType<Equal<NonNullable<typeof classified.assessed>["inputTokens"], number>>(true);

  // 2. Rewrite only the files that swallow errors, with ast-grep.
  const swallowing = classified.outputs
    .filter((output) => output.smell === "swallowed" && output.sure)
    .map((output) => output.path);
  await step(
    "Log swallowed errors",
    forEachFile({ include: swallowing, language: "typescript" }, (file) => {
      const root = parse(file).root();
      const edits = root
        .findAll({ rule: { pattern: "catch ($E) {}" } })
        .map((node) =>
          node.replace(
            `catch (${node.getMatch("E")?.text() ?? "error"}) { console.error(${node.getMatch("E")?.text() ?? "error"}); }`,
          ),
        );
      if (edits.length) file.write(root.commitEdits(edits));
    }),
  );

  // A function step may assess a file by path; the choice may name the provider.
  const one = await step("One file", async () => {
    const { answers, model } = await assess("src/index.ts", {
      questions: { kind: choice(["library", "app"]) },
      provider: "openai",
      model: "gpt-6-luna",
      context: { imports: 2, extra: ["README.md"], limit: 50_000 },
    });
    return { kind: answers.kind, model };
  });
  expectType<Equal<(typeof one)["kind"], "library" | "app">>(true);

  // @ts-expect-error only the two providers
  void assess("a.ts", { questions: QUESTIONS, provider: "gemini" });
  // @ts-expect-error a question is built with yesNo, choice, number or list
  void assess("a.ts", { questions: { ok: "Is it fine?" } });
}

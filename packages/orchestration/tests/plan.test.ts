import { describe, expect, it } from "vitest";
import { createHarness, failed } from "../src/harness.ts";
import { PlanValidationError, exec, guard, parallel, plan, workflow } from "../src/index.ts";

const Count = guard(
  "Count",
  (v: unknown): v is { count: number } => typeof v === "object" && v !== null,
);

const rename = exec({ name: "rename", command: "rename" });
const updateImports = exec({ name: "update-imports", command: "update-imports" });
const format = exec({ name: "format", command: "format" });
const countTodos = exec({
  name: "count-todos",
  command: "grep -c TODO",
  output: Count,
});
const countFixmes = exec({
  name: "count-fixmes",
  command: "grep -c FIXME",
  output: Count,
});

describe("plans", () => {
  it("runs an ordered plan and returns outputs in step order", async () => {
    const fixed = plan(rename, updateImports, format);
    expect(fixed.ir).toEqual({
      version: 1,
      steps: [
        { type: "run", id: "rename", name: "rename", kind: "exec" },
        {
          type: "run",
          id: "update-imports",
          name: "update-imports",
          kind: "exec",
        },
        { type: "run", id: "format", name: "format", kind: "exec" },
      ],
    });

    const h = createHarness({ fallback: (r) => `did ${r.commandId}` });
    const result = await h.run(fixed);
    expect(result.commands.map((c) => c.id)).toEqual(["rename", "update-imports", "format"]);
    expect(result.output).toEqual([
      { stdout: "did rename" },
      { stdout: "did update-imports" },
      { stdout: "did format" },
    ]);
    const typed: [{ stdout: string }, { stdout: string }, { stdout: string }] = result.output;
    expect(typed[2].stdout).toBe("did format");

    const replay = await h.reload({ fallback: () => failed("must not execute") }).run(fixed);
    expect(replay.replayed).toBe(true);
    expect(replay.output).toEqual(result.output);
  });

  it("runs explicit parallel groups and records members in group order", async () => {
    const survey = plan(parallel(countTodos, countFixmes), format);
    expect(survey.ir.steps[0]).toEqual({
      type: "parallel",
      members: [
        { id: "count-todos", name: "count-todos", kind: "exec" },
        { id: "count-fixmes", name: "count-fixmes", kind: "exec" },
      ],
    });

    const h = createHarness({
      results: { "count-todos": { count: 4 }, "count-fixmes": { count: 1 }, format: "" },
    });
    const result = await h.run(survey);
    expect(result.commands.map((c) => c.id)).toEqual(["count-todos", "count-fixmes", "format"]);
    const [counts] = result.output;
    expect(counts).toEqual([{ count: 4 }, { count: 1 }]);
    const typed: [[{ count: number }, { count: number }], { stdout: string }] = result.output;
    expect(typed[0][0].count + typed[0][1].count).toBe(5);
  });

  it("accepts mutations as an explicit independence assertion", () => {
    expect(parallel(countTodos, format).members).toEqual([countTodos, format]);
    expect(() => parallel()).toThrow(PlanValidationError);
  });

  it("rejects empty plans and duplicate runnable names", () => {
    expect(() => plan()).toThrow(PlanValidationError);
    expect(() => plan(format, rename, format)).toThrow(/appears twice/);
  });

  it("can be embedded in a procedural workflow", async () => {
    const wf = workflow(async (w) => {
      const [counts] = await w.run(plan(parallel(countTodos, countFixmes)));
      if (counts[0].count > 0) await w.run(format);
      return counts[0].count;
    });
    const h = createHarness({
      results: { "count-todos": { count: 2 }, "count-fixmes": { count: 0 }, format: "" },
    });
    const result = await h.run(wf);
    expect(result.output).toBe(2);
    expect(result.commands.map((c) => c.id)).toEqual(["count-todos", "count-fixmes", "format"]);
  });
});

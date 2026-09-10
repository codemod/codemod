import { describe, expect, it } from "vitest";
import { createHarness, failed } from "../src/harness.ts";
import { NondeterminismError, exec, workflow, type Workflow } from "../src/index.ts";

const a = exec({ name: "a", command: "a" });
const b = exec({ name: "b", command: "b" });
const c = exec({ name: "c", command: "c" });
const x = exec({ name: "x", command: "x" });

const sequence = (...steps: (typeof a)[]) =>
  workflow(async (w) => {
    for (const step of steps) await w.run(step);
    return steps.map((s) => s.name).join("");
  });

async function recorded(wf: Workflow<unknown>, finalize = true) {
  const h = createHarness({ fallback: () => "ok" });
  if (finalize) {
    await h.run(wf);
    return h;
  }
  // Record commands but stop before finalization by throwing from the body.
  const partial = workflow(async (w) => {
    await wf.body(w);
    throw new Error("crash before finalize");
  });
  await expect(h.run(partial)).rejects.toThrow("crash before finalize");
  return h;
}

async function replayError(wf: Workflow<unknown>, h: Awaited<ReturnType<typeof recorded>>) {
  const replay = h.reload({ fallback: () => failed("must not execute") });
  const error = await replay.run(wf).catch((e: unknown) => e);
  expect(error).toBeInstanceOf(NondeterminismError);
  expect(replay.executed).toHaveLength(0);
  return error as NondeterminismError;
}

describe("replay nondeterminism detection", () => {
  it("detects a changed command with the same id", async () => {
    const h = await recorded(sequence(a, b));
    const aChanged = exec({ name: "a", command: "a --different" });
    const error = await replayError(sequence(aChanged, b), h);
    expect(error.kind).toBe("changed");
    expect(error.detail).toMatchObject({ position: 0, expectedId: "a", actualId: "a" });
  });

  it("detects reordered commands", async () => {
    const h = await recorded(sequence(a, b, c));
    const error = await replayError(sequence(a, c, b), h);
    expect(error.kind).toBe("reordered");
    expect(error.detail.actualId).toBe("b");
  });

  it("detects a command removed from the middle", async () => {
    const h = await recorded(sequence(a, b, c));
    const error = await replayError(sequence(a, c), h);
    expect(error.kind).toBe("removed");
    expect(error.detail.missing).toEqual(["b"]);
  });

  it("detects a command removed from the middle before new work runs", async () => {
    const h = await recorded(sequence(a, b, c), false);
    const error = await replayError(sequence(a, c, x), h);
    expect(error.kind).toBe("removed");
    expect(error.detail.missing).toEqual(["b"]);
  });

  it("detects trailing commands that were removed", async () => {
    const h = await recorded(sequence(a, b, c));
    const error = await replayError(sequence(a, b), h);
    expect(error.kind).toBe("removed");
    expect(error.detail.missing).toEqual(["c"]);
  });

  it("detects a command added after finalization", async () => {
    const h = await recorded(sequence(a, b));
    const error = await replayError(sequence(a, b, c), h);
    expect(error.kind).toBe("added");
    expect(error.detail.actualId).toBe("c");
  });

  it("detects a replaced command in unfinalized history", async () => {
    const h = await recorded(sequence(a, b), false);
    const error = await replayError(sequence(a, x), h);
    expect(error.kind).toBe("changed");
    expect(error.detail).toMatchObject({ position: 1, expectedId: "b", actualId: "x" });
  });

  it("detects a different final output", async () => {
    const h = await recorded(sequence(a, b));
    const differentOutput = workflow(async (w) => {
      await w.run(a);
      await w.run(b);
      return "something else";
    });
    const error = await replayError(differentOutput, h);
    expect(error.kind).toBe("output");
  });

  it("continues an unfinalized history by replaying then executing only new commands", async () => {
    const h = await recorded(sequence(a, b), false);
    const resume = h.reload({
      results: { a: failed("must not execute"), b: failed("must not execute"), c: "fresh" },
    });
    const result = await resume.run(sequence(a, b, c));
    expect(result.output).toBe("abc");
    expect(resume.executed.map((r) => r.commandId)).toEqual(["c"]);
    expect(result.history.events.at(-1)).toEqual({ type: "finalized", output: "abc" });
  });
});

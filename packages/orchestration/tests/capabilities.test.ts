/**
 * The capability contract: a runtime that lacks a capability refuses it
 * fast; `quickjs` has every one.
 */
import { describe, expect, it } from "vitest";
import { createHarness } from "./support/harness.ts";
import {
  OperationError,
  UnsupportedInRuntimeError,
  agent,
  assessment,
  dynamic,
  jssg,
  parallel,
  sequence,
  shell,
  type RuntimeDescriptor,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

describe("the capability contract", () => {
  const lint = shell({ name: "lint", command: "lint" });
  const fix = agent({ name: "fix", prompt: "fix it" });
  const judge = assessment({
    name: "judge",
    include: ["**/*.ts"],
    ask: () => ({ ok: { type: "yesno" as const, instructions: "ok?" } }),
  });
  const semantic = jssg({
    name: "semantic",
    language: "typescript",
    transform: ref("semantic"),
    semanticAnalysis: "file",
  });
  // The contract applies to any runtime; the built-in ones support every
  // kind, so a limited one exercises the refusals.
  const limited: RuntimeDescriptor = {
    name: "limited",
    capabilities: {
      operations: { shell: true, jssg: true, agent: false, assessment: true },
      semanticAnalysis: false,
    },
  };

  it("refuses unsupported static operations before anything runs", async () => {
    for (const [root, kind, id] of [
      [sequence(lint(), fix()), "agent", "fix"],
      [parallel(judge(), fix()), "agent", "fix"],
      [fix, "agent", "fix"],
    ] as const) {
      const h = createHarness({ runtime: limited, fallback: () => "ok" });
      const run = (h.run as (root: unknown) => Promise<unknown>)(root);
      await expect(run).rejects.toThrow(UnsupportedInRuntimeError);
      await expect(run).rejects.toThrow(
        `operation kind '${kind}' is unsupported in runtime 'limited' (command '${id}')`,
      );
      expect(h.executed).toEqual([]);
      expect((await h.store.load()).events).toEqual([]);
    }
    const bare = createHarness({ runtime: limited, fallback: () => [] });
    await expect(bare.run(semantic)).rejects.toThrow(
      "JSSG semantic analysis is unsupported in runtime 'limited' (command 'semantic')",
    );
  });

  it.each([
    ["parallel shell and assessment", parallel(lint(), judge()), ["judge", "lint"]],
    ["sequence shell then agent", sequence(lint(), fix()), ["fix", "lint"]],
    ["static semantic jssg", semantic, ["semantic"]],
    ["dynamic semantic jssg", dynamic(async () => semantic()), ["semantic"]],
  ] as const)("lets every kind through on quickjs: %s", async (_name, root, ids) => {
    const h = createHarness({ runtime: "quickjs", fallback: () => [] });
    await (h.run as (root: unknown) => Promise<unknown>)(root);
    expect(h.executed.map((request) => request.commandId).sort()).toEqual(ids);
  });

  it("fails dynamically issued unsupported operations fast, without executing them", async () => {
    const h = createHarness({ runtime: limited, fallback: () => "ok" });
    const workflow = dynamic(async () => {
      await lint();
      await fix();
    });
    const error = await h.run(workflow).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(OperationError);
    expect((error as Error).message).toContain(
      "operation kind 'agent' is unsupported in runtime 'limited'",
    );
    expect(h.executed.map((request) => request.commandId)).toEqual(["lint"]);
  });

  it("lets every kind through without a runtime", async () => {
    const h = createHarness({ fallback: () => "ok" });
    await h.run(sequence(lint(), fix()));
    expect(h.executed.map((request) => request.commandId)).toEqual(["lint", "fix"]);
  });
});

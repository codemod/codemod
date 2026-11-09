/**
 * Runtime selection: `--runtime` parsing, the registry contract (a provider
 * launches the whole workflow out of process), the `quickjs` availability
 * check the benchmark harness probes for, and the capability contract (a
 * runtime that lacks a capability refuses it fast; `quickjs` has every one).
 */
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { parseArgs, runWorkflowCli, USAGE } from "../src/host/cli.ts";
import { createHarness } from "../src/host/harness.ts";
import { defaultQuickjsHostPath } from "../src/host/runtimes/quickjs.ts";
import {
  DEFAULT_RUNTIME,
  OperationError,
  QUICKJS_RUNTIME,
  RuntimeRegistry,
  UnsupportedInRuntimeError,
  agent,
  assessment,
  defaultRuntimes,
  dynamic,
  jssg,
  parallel,
  runWorkflow,
  sequence,
  shell,
  type LaunchRequest,
  type RuntimeDescriptor,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

const fixtures = resolve(import.meta.dirname, "fixtures");
const shellWorkflow = resolve(fixtures, "load/shell.ts");
const bin = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");
const missingHost = "/nonexistent/orchestration-quickjs-host";

/** A registry whose one runtime records each launch instead of running it. */
function recordingRegistry() {
  const launches: LaunchRequest[] = [];
  const registry = new RuntimeRegistry([
    {
      name: "remote",
      capabilities: QUICKJS_RUNTIME.capabilities,
      description: "test",
      availability: () => ({ available: true }),
      launch: async (request) => {
        launches.push(request);
        return { launched: true };
      },
    },
  ]);
  return { registry, launches };
}

describe("--runtime parsing", () => {
  it("accepts an optional run subcommand and --runtime in both spellings", () => {
    expect(parseArgs(["run", "wf.ts", "--runtime", "quickjs"])).toMatchObject({
      workflow: resolve("wf.ts"),
      runtime: "quickjs",
    });
    // Options may precede the workflow (the benchmark harness's form).
    expect(parseArgs(["run", "--runtime=quickjs", "wf.ts", "--target", "repo"])).toMatchObject({
      workflow: resolve("wf.ts"),
      runtime: "quickjs",
      target: resolve("repo"),
    });
    expect(() => parseArgs(["run", "a.ts", "b.ts"])).toThrow(/unexpected argument: b\.ts/u);
    expect(parseArgs(["wf.ts", "--target=repo"]).target).toBe(resolve("repo"));
    expect(parseArgs(["wf.ts"])).not.toHaveProperty("runtime");
    expect(() => parseArgs(["run"])).toThrow(USAGE);
    expect(() => parseArgs(["wf.ts", "--runtime"])).toThrow(/missing value for --runtime/u);
    expect(USAGE).toContain("--runtime <quickjs>");
  });
});

describe("the runtime registry", () => {
  it("has quickjs as its only and default runtime", () => {
    expect(defaultRuntimes.names()).toEqual(["quickjs"]);
    expect(DEFAULT_RUNTIME).toBe("quickjs");
    for (const removed of ["node", "bridge", "deno"]) {
      expect(() => defaultRuntimes.resolve(removed)).toThrow(
        `unknown runtime '${removed}'; expected one of: quickjs`,
      );
    }
  });

  it("reports quickjs available exactly when its host binary is built", () => {
    const quickjs = defaultRuntimes.resolve("quickjs").availability({});
    expect(quickjs.available).toBe(existsSync(defaultQuickjsHostPath()));
    // The full verification builds it; CODEMOD_REQUIRE_RUNTIMES=1 makes that a hard check.
    if (process.env.CODEMOD_REQUIRE_RUNTIMES === "1") expect(quickjs).toEqual({ available: true });
  });

  it("refuses runtime 'quickjs' before building anything when its host binary is missing", async () => {
    const previous = process.env.CODEMOD_QUICKJS_HOST_BIN;
    process.env.CODEMOD_QUICKJS_HOST_BIN = missingHost;
    try {
      const reason = `quickjs host binary not found at ${missingHost}`;
      await expect(runWorkflowCli(["run", shellWorkflow, "--target", fixtures])).rejects.toThrow(
        reason,
      );
      await expect(runWorkflow(shellWorkflow, { target: fixtures })).rejects.toThrow(reason);
    } finally {
      if (previous === undefined) delete process.env.CODEMOD_QUICKJS_HOST_BIN;
      else process.env.CODEMOD_QUICKJS_HOST_BIN = previous;
    }
  });

  it("hands a registered provider the whole launch", async () => {
    const { registry, launches } = recordingRegistry();
    const output = await runWorkflow(shellWorkflow, {
      runtime: "remote",
      target: fixtures,
      env: { EXTRA: "1" },
      input: null,
      registry,
    });
    expect(output).toEqual({ launched: true });
    expect(launches).toEqual([
      { workflow: shellWorkflow, target: fixtures, settings: { env: { EXTRA: "1" } }, input: null },
    ]);
  });

  it("passes --input from the CLI to the launch, keeping explicit null apart from no input", async () => {
    const { registry, launches } = recordingRegistry();
    const cli = (...extra: string[]) =>
      runWorkflowCli([shellWorkflow, "--runtime=remote", ...extra], undefined, () => {}, {
        registry,
      });
    await cli();
    await cli("--input", "null");
    await cli("--input", '{"name":"x"}');
    expect(launches.map((launch) => ("input" in launch ? launch.input : "absent"))).toEqual([
      "absent",
      null,
      { name: "x" },
    ]);
  });

  it.skipIf(process.platform === "win32")(
    "codemod-workflow exits nonzero with the reason for an unavailable runtime",
    () => {
      const result = spawnSync(
        process.execPath,
        [bin, "run", shellWorkflow, "--runtime=quickjs", "--target", fixtures],
        { encoding: "utf8", env: { ...process.env, CODEMOD_QUICKJS_HOST_BIN: missingHost } },
      );
      expect(result.status, result.stderr).toBe(1);
      expect(result.stderr).toContain(`quickjs host binary not found at ${missingHost}`);
      expect(result.stdout).toBe("");
    },
  );
});

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

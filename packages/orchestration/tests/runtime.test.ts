/**
 * Runtime selection: `--runtime` parsing, the registry contract (in-process
 * `createExecutor` vs out-of-process `launch`), the `node` and `quickjs`
 * availability checks the benchmark harness probes for, and the capability
 * contract (a runtime that lacks a capability refuses it fast; `node` and
 * `quickjs` have every one).
 */
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { isAvailable as isNapiAvailable } from "@codemod.com/jssg-napi";
import { parseArgs, runWorkflowCli, USAGE } from "../src/host/cli.ts";
import { createHarness } from "../src/host/harness.ts";
import { defaultQuickjsHostPath } from "../src/host/runtimes/quickjs.ts";
import {
  DEFAULT_RUNTIME,
  NODE_RUNTIME,
  OperationError,
  PROTOCOL_VERSION,
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
  type OperationRequest,
  type RuntimeDescriptor,
  type RuntimeProvider,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

const fixtures = resolve(import.meta.dirname, "fixtures");
const shellWorkflow = resolve(fixtures, "load/shell.ts");
const bin = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");

describe("--runtime parsing", () => {
  it("accepts an optional run subcommand and --runtime in both spellings", () => {
    expect(parseArgs(["run", "wf.ts", "--runtime", "node"])).toMatchObject({
      workflow: resolve("wf.ts"),
      runtime: "node",
    });
    expect(parseArgs(["run", "wf.ts", "--runtime=quickjs"]).runtime).toBe("quickjs");
    // Options may precede the workflow (the benchmark harness's form).
    expect(parseArgs(["run", "--runtime=node", "wf.ts", "--target", "repo"])).toMatchObject({
      workflow: resolve("wf.ts"),
      runtime: "node",
      target: resolve("repo"),
    });
    expect(() => parseArgs(["run", "a.ts", "b.ts"])).toThrow(/unexpected argument: b\.ts/u);
    expect(parseArgs(["wf.ts", "--target=repo"]).target).toBe(resolve("repo"));
    expect(parseArgs(["wf.ts"])).not.toHaveProperty("runtime");
    expect(() => parseArgs(["run"])).toThrow(USAGE);
    expect(() => parseArgs(["wf.ts", "--runtime"])).toThrow(/missing value for --runtime/u);
    expect(USAGE).toContain("--runtime <node|quickjs>");
  });
});

describe("the runtime registry", () => {
  it("lists node (default) and quickjs", () => {
    expect(defaultRuntimes.names()).toEqual(["node", "quickjs"]);
    expect(DEFAULT_RUNTIME).toBe("node");
    for (const removed of ["bridge", "deno"]) {
      expect(() => defaultRuntimes.resolve(removed)).toThrow(
        `unknown runtime '${removed}'; expected one of: node, quickjs`,
      );
    }
  });

  it("reports node and quickjs available exactly when their addon / host binary is built", () => {
    const settings = {};
    const node = defaultRuntimes.resolve("node").availability(settings);
    const quickjs = defaultRuntimes.resolve("quickjs").availability(settings);
    expect(node.available).toBe(isNapiAvailable());
    expect(quickjs.available).toBe(existsSync(defaultQuickjsHostPath()));
    // Neither is a placeholder any more.
    for (const availability of [node, quickjs]) {
      if (!availability.available) expect(availability.reason).not.toContain("not available yet");
    }
    // The full verification builds both; CODEMOD_REQUIRE_RUNTIMES=1 makes that a hard check.
    if (process.env.CODEMOD_REQUIRE_RUNTIMES === "1") {
      expect(node).toEqual({ available: true });
      expect(quickjs).toEqual({ available: true });
    }
  });

  it("refuses runtime 'quickjs' before loading anything when its host binary is missing", async () => {
    const previous = process.env.CODEMOD_QUICKJS_HOST_BIN;
    process.env.CODEMOD_QUICKJS_HOST_BIN = "/nonexistent/orchestration-quickjs-host";
    try {
      const reason = "quickjs host binary not found at /nonexistent/orchestration-quickjs-host";
      await expect(
        runWorkflowCli(["run", shellWorkflow, "--runtime", "quickjs", "--target", fixtures]),
      ).rejects.toThrow(reason);
      await expect(
        runWorkflow(shellWorkflow, { runtime: "quickjs", target: fixtures }),
      ).rejects.toThrow(reason);
    } finally {
      if (previous === undefined) delete process.env.CODEMOD_QUICKJS_HOST_BIN;
      else process.env.CODEMOD_QUICKJS_HOST_BIN = previous;
    }
  });

  it.skipIf(process.platform === "win32" || !isNapiAvailable())(
    "runs the default runtime (node) through the registry",
    async () => {
      expect(await runWorkflow(shellWorkflow, { target: fixtures })).toEqual({ stdout: "" });
    },
  );

  it("uses a registered in-process provider without any CLI change", async () => {
    const requests: OperationRequest[] = [];
    const provider: RuntimeProvider = {
      name: "scripted",
      capabilities: NODE_RUNTIME.capabilities,
      description: "test",
      availability: () => ({ available: true }),
      createExecutor: (context) => ({
        async execute(request) {
          requests.push(request);
          expect(context.cwd).toBe(fixtures);
          return {
            protocolVersion: PROTOCOL_VERSION,
            commandId: request.commandId,
            status: "succeeded",
            output: { stdout: "scripted" },
          };
        },
      }),
    };
    const registry = new RuntimeRegistry([provider]);
    const output = await runWorkflowCli(
      ["run", shellWorkflow, "--runtime=scripted", "--target", fixtures],
      undefined,
      () => {},
      { registry },
    );
    expect(output).toEqual({ stdout: "scripted" });
    expect(requests.map((request) => request.commandId)).toEqual(["inspect"]);
  });

  it("hands an out-of-process provider the whole launch", async () => {
    const launches: unknown[] = [];
    const registry = new RuntimeRegistry([
      {
        name: "remote",
        capabilities: NODE_RUNTIME.capabilities,
        description: "test",
        availability: () => ({ available: true }),
        launch: async (request) => {
          launches.push(request);
          return { launched: true };
        },
      },
    ]);
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
    await expect(
      runWorkflowCli(
        [shellWorkflow, "--runtime", "remote", "--target", fixtures, "--dashboard"],
        undefined,
        () => {},
        { registry },
      ),
    ).rejects.toThrow("--dashboard is not supported by runtime 'remote'");
  });

  it.skipIf(process.platform === "win32")(
    "codemod-workflow exits nonzero with the reason for an unavailable runtime",
    () => {
      for (const [name, reason, env] of [
        [
          "quickjs",
          "quickjs host binary not found at /nonexistent/orchestration-quickjs-host",
          { CODEMOD_QUICKJS_HOST_BIN: "/nonexistent/orchestration-quickjs-host" },
        ],
        [
          "node",
          "runtime 'node' needs the jssg napi addon, not found at /nonexistent/addon.node",
          { CODEMOD_JSSG_NAPI: "/nonexistent/addon.node" },
        ],
      ] as const) {
        const result = spawnSync(
          process.execPath,
          [bin, "run", shellWorkflow, `--runtime=${name}`, "--target", fixtures],
          { encoding: "utf8", env: { ...process.env, ...env } },
        );
        expect(result.status, result.stderr).toBe(1);
        expect(result.stderr).toContain(reason);
        expect(result.stdout).toBe("");
      }
    },
  );

  it("refuses runtime 'node' before loading anything when the addon is missing", async () => {
    const saved = process.env.CODEMOD_JSSG_NAPI;
    process.env.CODEMOD_JSSG_NAPI = "/nonexistent/addon.node";
    try {
      await expect(
        runWorkflow(shellWorkflow, { runtime: "node", target: fixtures }),
      ).rejects.toThrow(
        "runtime 'node' needs the jssg napi addon, not found at /nonexistent/addon.node; build it with 'pnpm --filter @codemod.com/jssg-napi build' or set CODEMOD_JSSG_NAPI",
      );
    } finally {
      if (saved === undefined) delete process.env.CODEMOD_JSSG_NAPI;
      else process.env.CODEMOD_JSSG_NAPI = saved;
    }
  });

  it.skipIf(process.platform === "win32" || !isNapiAvailable())(
    "runs a shell workflow in-process on runtime 'node'",
    async () => {
      const output = await runWorkflow(shellWorkflow, { runtime: "node", target: fixtures });
      expect(output).toEqual({ stdout: "" });
    },
  );
});

describe("the capability contract", () => {
  const lint = shell({ name: "lint", command: "lint" });
  const fix = agent({ name: "fix", prompt: "fix it" });
  const judge = assessment({
    name: "judge",
    include: ["**/*.ts"],
    ask: () => ({ ok: { type: "noul" as const, instructions: "ok?" } }),
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

  it("lets every kind through on node and quickjs", async () => {
    for (const runtime of ["node", "quickjs"] as const) {
      for (const [root, ids] of [
        [parallel(lint(), judge()), ["judge", "lint"]],
        [sequence(lint(), fix()), ["fix", "lint"]],
        [semantic, ["semantic"]],
        [dynamic(async () => semantic()), ["semantic"]],
      ] as const) {
        const h = createHarness({ runtime, fallback: () => [] });
        await (h.run as (root: unknown) => Promise<unknown>)(root);
        expect(h.executed.map((request) => request.commandId).sort()).toEqual(ids);
      }
    }
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

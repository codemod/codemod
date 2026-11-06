/**
 * Runtime selection: `--runtime` parsing, the registry contract (in-process
 * `createExecutor` vs out-of-process `launch`), the placeholder `node` and
 * `quickjs` runtimes the benchmark harness probes for, and the capability
 * contract (`agent`/`assessment`/semantic analysis fail fast outside `bridge`).
 */
import { spawnSync } from "node:child_process";
import { chmodSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { parseArgs, runWorkflowCli, USAGE } from "../src/host/cli.ts";
import { createHarness } from "../src/host/harness.ts";
import {
  BRIDGE_RUNTIME,
  NODE_RUNTIME,
  OperationError,
  PROTOCOL_VERSION,
  RuntimeRegistry,
  UnsupportedInRuntimeError,
  agent,
  assessment,
  bridgeRuntime,
  defaultRuntimes,
  dynamic,
  jssg,
  parallel,
  runWorkflow,
  sequence,
  shell,
  type OperationRequest,
  type RuntimeProvider,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

const fixtures = resolve(import.meta.dirname, "fixtures");
const shellWorkflow = resolve(fixtures, "load/shell.ts");
const fakeBridge = resolve(fixtures, "fake-bridge.mjs");
const bin = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");

describe("--runtime parsing", () => {
  it("accepts an optional run subcommand and --runtime in both spellings", () => {
    expect(parseArgs(["run", "wf.ts", "--runtime", "node"])).toMatchObject({
      workflow: resolve("wf.ts"),
      runtime: "node",
    });
    expect(parseArgs(["run", "wf.ts", "--runtime=quickjs"]).runtime).toBe("quickjs");
    expect(parseArgs(["wf.ts", "--target=repo"]).target).toBe(resolve("repo"));
    expect(parseArgs(["wf.ts"])).not.toHaveProperty("runtime");
    expect(() => parseArgs(["run"])).toThrow(USAGE);
    expect(() => parseArgs(["wf.ts", "--runtime"])).toThrow(/missing value for --runtime/u);
    expect(USAGE).toContain("--runtime <bridge|node|quickjs>");
  });
});

describe("the runtime registry", () => {
  it("lists bridge (default), node, and quickjs", () => {
    expect(defaultRuntimes.names()).toEqual(["bridge", "node", "quickjs"]);
    expect(defaultRuntimes.resolve("bridge")).toBe(bridgeRuntime);
    expect(() => defaultRuntimes.resolve("deno")).toThrow(
      "unknown runtime 'deno'; expected one of: bridge, node, quickjs",
    );
  });

  it.each(["node", "quickjs"])(
    "refuses runtime '%s' before loading anything: not available yet",
    async (name) => {
      await expect(
        runWorkflowCli(["run", shellWorkflow, "--runtime", name, "--target", fixtures]),
      ).rejects.toThrow(`runtime '${name}' not available yet`);
      await expect(runWorkflow(shellWorkflow, { runtime: name, target: fixtures })).rejects.toThrow(
        `runtime '${name}' not available yet`,
      );
    },
  );

  it("keeps the bridge runtime's missing-binary error", async () => {
    await expect(
      runWorkflowCli([shellWorkflow, "--target", fixtures, "--bridge", "/nonexistent/bridge"]),
    ).rejects.toThrow("bridge binary not found at /nonexistent/bridge");
  });

  it.skipIf(process.platform === "win32")(
    "runs the default runtime through the registry exactly as before",
    async () => {
      chmodSync(fakeBridge, 0o755);
      const output = (await runWorkflow(shellWorkflow, {
        target: fixtures,
        bridge: fakeBridge,
      })) as { stdout: string };
      const echoed = JSON.parse(output.stdout) as { request: OperationRequest };
      expect(echoed.request).toMatchObject({
        commandId: "inspect",
        operation: { kind: "shell", command: "true" },
      });
    },
  );

  it("uses a registered in-process provider without any CLI change", async () => {
    const requests: OperationRequest[] = [];
    const provider: RuntimeProvider = {
      name: "scripted",
      capabilities: BRIDGE_RUNTIME.capabilities,
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
    const registry = new RuntimeRegistry([bridgeRuntime, provider]);
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
      bridge: "/b",
      input: null,
      registry,
    });
    expect(output).toEqual({ launched: true });
    expect(launches).toEqual([
      { workflow: shellWorkflow, target: fixtures, settings: { bridge: "/b" }, input: null },
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
      for (const name of ["node", "quickjs"]) {
        const result = spawnSync(
          process.execPath,
          [bin, "run", shellWorkflow, `--runtime=${name}`, "--target", fixtures],
          { encoding: "utf8" },
        );
        expect(result.status, result.stderr).toBe(1);
        expect(result.stderr).toContain(`runtime '${name}' not available yet`);
        expect(result.stdout).toBe("");
      }
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

  it("refuses unsupported static operations before anything runs", async () => {
    for (const [root, kind, id] of [
      [sequence(lint(), fix()), "agent", "fix"],
      [parallel(lint(), judge()), "assessment", "judge"],
      [fix, "agent", "fix"],
    ] as const) {
      const h = createHarness({ runtime: "node", fallback: () => "ok" });
      const run = (h.run as (root: unknown) => Promise<unknown>)(root);
      await expect(run).rejects.toThrow(UnsupportedInRuntimeError);
      await expect(run).rejects.toThrow(
        `operation kind '${kind}' is unsupported in runtime 'node' (command '${id}')`,
      );
      expect(h.executed).toEqual([]);
      expect((await h.store.load()).events).toEqual([]);
    }
    const bare = createHarness({ runtime: "quickjs", fallback: () => [] });
    await expect(bare.run(semantic)).rejects.toThrow(
      "JSSG semantic analysis is unsupported in runtime 'quickjs' (command 'semantic')",
    );
  });

  it("fails dynamically issued unsupported operations fast, without executing them", async () => {
    const h = createHarness({ runtime: "node", fallback: () => "ok" });
    const workflow = dynamic(async () => {
      await lint();
      await fix();
    });
    const error = await h.run(workflow).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(OperationError);
    expect((error as Error).message).toContain(
      "operation kind 'agent' is unsupported in runtime 'node'",
    );
    expect(h.executed.map((request) => request.commandId)).toEqual(["lint"]);

    const semanticRun = createHarness({ runtime: "node", fallback: () => [] });
    await expect(semanticRun.run(dynamic(async () => semantic()))).rejects.toThrow(
      "JSSG semantic analysis is unsupported in runtime 'node'",
    );
    expect(semanticRun.executed).toEqual([]);
  });

  it("lets every kind through on the bridge runtime and without a runtime", async () => {
    for (const runtime of ["bridge", undefined] as const) {
      const h = createHarness({ ...(runtime ? { runtime } : {}), fallback: () => "ok" });
      await h.run(sequence(lint(), fix()));
      expect(h.executed.map((request) => request.commandId)).toEqual(["lint", "fix"]);
    }
  });
});

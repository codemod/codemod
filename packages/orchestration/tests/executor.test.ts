/**
 * Script identity and resolution: a JSSG `script` is a safe relative path on
 * the wire and in history, and the executor (not the operation) carries the
 * machine-specific root it resolves against.
 */
import { chmodSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { parseArgs, USAGE } from "../src/cli.ts";
import { createHarness } from "../src/harness.ts";
import {
  BridgeExecutor,
  MemoryHistoryStore,
  isOperationRequest,
  isSafeRelativePath,
  jssg,
  run,
  workflow,
  type OperationRequest,
} from "../src/index.ts";

const fakeBridge = resolve(import.meta.dirname, "fixtures/fake-bridge.mjs");

describe("safe relative paths", () => {
  it.each([
    ["scripts/migrate.ts", true],
    ["scripts/foo..bar.ts", true],
    ["a..b/c", true],
    ["", false],
    ["  ", false],
    ["/abs/x.ts", false],
    ["\\x.ts", false],
    ["\\\\server\\x.ts", false],
    ["C:\\x.ts", false],
    ["c:/x.ts", false],
    ["../x.ts", false],
    ["scripts/../x.ts", false],
    ["scripts\\..\\x.ts", false],
  ])("%j -> %s", (path, ok) => {
    expect(isSafeRelativePath(path)).toBe(ok);
  });
});

describe("jssg definitions", () => {
  it("accept relative scripts and reject absolute or escaping ones", () => {
    expect(jssg({ name: "ok", script: "scripts/a..b.ts", language: "typescript" }).script).toBe(
      "scripts/a..b.ts",
    );
    for (const script of ["/abs/x.ts", "C:\\x.ts", "../x.ts", "scripts/../x.ts"]) {
      expect(() => jssg({ name: "bad", script, language: "typescript" })).toThrow(
        /must be a relative path without '\.\.' segments/,
      );
    }
    expect(() => jssg({ name: "bad", script: " ", language: "typescript" })).toThrow(
      /must not be empty/,
    );
  });

  it("validate semanticAnalysis.root with the same path rules", () => {
    const define = (root: string) =>
      jssg({
        name: "x",
        script: "x.ts",
        language: "typescript",
        semanticAnalysis: { mode: "workspace", root },
      });
    expect(define("src..gen").toOperation(undefined).semanticAnalysis).toEqual({
      mode: "workspace",
      root: "src..gen",
    });
    for (const root of ["/src", "C:\\src", "..", "src/../..", " "]) {
      expect(() => define(root), root).toThrow(/must be a safe relative path/);
    }
    expect(() =>
      jssg({
        name: "x",
        script: "x.ts",
        language: "typescript",
        semanticAnalysis: { mode: "file", root: "src" },
      }),
    ).toThrow(/requires workspace mode/);
  });
});

const migrate = jssg({ name: "migrate", script: "scripts/migrate.ts", language: "typescript" });
const wf = workflow(() => migrate({ target: { root: "apps/web" } }));

describe.skipIf(process.platform === "win32")("BridgeExecutor script root", () => {
  it("sends scriptRoot as request context, outside the operation and history", async () => {
    chmodSync(fakeBridge, 0o755);
    const scriptRoot = resolve(import.meta.dirname, "fixtures");
    const executor = new BridgeExecutor({ bin: fakeBridge, scriptRoot });
    const store = new MemoryHistoryStore();

    const first = await run(wf, { executor, history: store });

    const { request } = first.output as { request: OperationRequest };
    expect(isOperationRequest(request)).toBe(true);
    expect(request.context).toEqual({ scriptRoot });
    expect(request.operation).toEqual({
      kind: "jssg",
      script: "scripts/migrate.ts",
      language: "typescript",
      target: { root: "apps/web" },
    });
    const scheduled = first.history.events.find((event) => event.type === "scheduled");
    expect(scheduled).toEqual({
      type: "scheduled",
      command: {
        id: "migrate",
        runnable: "migrate",
        kind: "jssg",
        operation: request.operation,
      },
    });
    // Only the completion (which this fake bridge fills with the echoed request)
    // mentions the root; the recorded command that replay compares does not.
    expect(JSON.stringify(scheduled)).not.toContain("scriptRoot");
    expect(JSON.stringify(scheduled)).not.toContain(scriptRoot);
  });

  it("replays history recorded on another checkout without re-executing", async () => {
    chmodSync(fakeBridge, 0o755);
    const recorded = new BridgeExecutor({ bin: fakeBridge, scriptRoot: "/checkout/one" });
    const store = new MemoryHistoryStore();
    const first = await run(wf, { executor: recorded, history: store });

    const executed: OperationRequest[] = [];
    const elsewhere = new BridgeExecutor({ bin: fakeBridge, scriptRoot: "/checkout/two" });
    const second = await run(wf, {
      executor: {
        execute(request) {
          executed.push(request);
          return elsewhere.execute(request);
        },
      },
      history: MemoryHistoryStore.fromJSON(store.serialize()),
    });

    expect(second.replayed).toBe(true);
    expect(second.output).toEqual(first.output);
    expect(executed).toHaveLength(0);
  });

  it("omits context when no scriptRoot is configured", async () => {
    chmodSync(fakeBridge, 0o755);
    const h = createHarness();
    const executor = new BridgeExecutor({ bin: fakeBridge });
    const result = await run(wf, { executor, history: h.store });
    const { request } = result.output as { request: OperationRequest };
    expect(request).not.toHaveProperty("context");
  });
});

describe("codemod-workflow argument parsing", () => {
  it("defaults the script root to the workflow directory and resolves every path", () => {
    const options = parseArgs(["fixtures/jssg/workflow.ts"]);
    expect(options.workflow).toBe(resolve("fixtures/jssg/workflow.ts"));
    expect(options.scriptRoot).toBe(resolve("fixtures/jssg"));
    expect(options.target).toBe(process.cwd());
    expect(options.bridge).toBe(
      process.env.CODEMOD_BRIDGE_BIN === undefined
        ? resolve(import.meta.dirname, "../../../target/debug/butterflow-execution-bridge")
        : resolve(process.env.CODEMOD_BRIDGE_BIN),
    );
  });

  it("accepts explicit target, script root, and bridge", () => {
    const options = parseArgs([
      "wf.ts",
      "--target",
      "repo",
      "--script-root",
      "pkg/scripts",
      "--bridge",
      "bin/bridge",
    ]);
    expect(options).toEqual({
      workflow: resolve("wf.ts"),
      target: resolve("repo"),
      scriptRoot: resolve("pkg/scripts"),
      bridge: resolve("bin/bridge"),
    });
  });

  it("rejects missing workflow, unknown options, and dangling values", () => {
    expect(() => parseArgs([])).toThrow(USAGE);
    expect(() => parseArgs(["--target", "x"])).toThrow(USAGE);
    expect(() => parseArgs(["wf.ts", "--nope", "x"])).toThrow(/unknown option: --nope/);
    expect(() => parseArgs(["wf.ts", "--target"])).toThrow(/missing value for --target/);
  });
});

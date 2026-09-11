/**
 * Artifact identity through the runtime: a JSSG command records the transform
 * as `{ name, hash }`, the executor supplies the source in the request
 * context only, and a history replays on any checkout. Also the local CLI's
 * argument parsing.
 */
import { chmodSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { parseArgs, USAGE } from "../src/cli.ts";
import {
  BridgeExecutor,
  CollectingSink,
  MemoryHistoryStore,
  OperationError,
  exec,
  isSafeRelativePath,
  jssg,
  run,
  workflow,
  type OperationRequest,
} from "../src/index.ts";
import { artifact, ref } from "./helpers.ts";

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
  it("refuse an unbuilt inline transform and malformed references", () => {
    expect(() => jssg({ name: "inline", language: "typescript", transform: () => null })).toThrow(
      /jssg 'inline': inline transform was not extracted; run the workflow with codemod-workflow/u,
    );
    for (const transform of [
      { name: "x", hash: "abc" },
      { name: "x", hash: "A".repeat(64) },
      { name: " ", hash: "a".repeat(64) },
      { name: "x", hash: "a".repeat(64), source: "x" },
    ]) {
      expect(() =>
        jssg({ name: "x", language: "typescript", transform: transform as never }),
      ).toThrow(/must be the \{ name, hash \} reference the build produced/u);
    }
  });

  it("validate selector data and semanticAnalysis.root", () => {
    const define = (selector: unknown) =>
      jssg({ name: "x", language: "typescript", transform: ref("x"), selector: selector as never });
    expect(
      define({ rule: { pattern: "a($B)" }, constraints: { B: { kind: "string" } } }).selector,
    ).toEqual({ rule: { pattern: "a($B)" }, constraints: { B: { kind: "string" } } });
    expect(define(undefined)).not.toHaveProperty("selector");
    for (const bad of [{}, { rule: {} }, { rule: "a" }, { rule: { pattern: "a" }, id: "s" }, "a"]) {
      expect(() => define(bad), JSON.stringify(bad)).toThrow(
        /selector must be JSON with a non-empty 'rule'/u,
      );
    }

    const semantic = (root: string) =>
      jssg({
        name: "x",
        language: "typescript",
        transform: ref("x"),
        semanticAnalysis: { mode: "workspace", root },
      });
    expect(semantic("src..gen").toOperation(undefined).semanticAnalysis).toEqual({
      mode: "workspace",
      root: "src..gen",
    });
    for (const root of ["/src", "C:\\src", "..", "src/../..", " "]) {
      expect(() => semantic(root), root).toThrow(/must be a safe relative path/u);
    }
    expect(() =>
      jssg({
        name: "x",
        language: "typescript",
        transform: ref("x"),
        semanticAnalysis: { mode: "file", root: "src" },
      }),
    ).toThrow(/requires workspace mode/u);
  });
});

const SOURCE = "export default async function transform(root) { return null; }\n";
const built = artifact("migrate", SOURCE);
const migrate = jssg({
  name: "migrate",
  language: "typescript",
  transform: { name: built.name, hash: built.hash },
});
const wf = workflow(() => migrate({ target: { root: "apps/web" } }));

interface EchoOutput {
  path: string;
  context: { targetRoot: string; artifact: { source: string } };
}

describe.skipIf(process.platform === "win32")("BridgeExecutor artifacts", () => {
  let repo: string;
  beforeEach(() => {
    chmodSync(fakeBridge, 0o755);
    repo = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-exec-")));
    mkdirSync(join(repo, "apps/web"), { recursive: true });
    writeFileSync(join(repo, "apps/web/a.ts"), "a\n");
  });
  afterEach(() => rmSync(repo, { recursive: true, force: true }));

  it("records the reference in history and sends the source only in the request context", async () => {
    const executor = new BridgeExecutor({
      bin: fakeBridge,
      cwd: repo,
      artifacts: new Map([[built.hash, built]]),
    });
    const store = new MemoryHistoryStore();

    const first = await run(wf, { executor, history: store });

    const [output] = first.output as EchoOutput[];
    expect(output).toMatchObject({
      path: "a.ts",
      context: { targetRoot: join(repo, "apps/web"), artifact: { source: SOURCE } },
    });
    const scheduled = first.history.events.find((event) => event.type === "scheduled");
    expect(scheduled).toEqual({
      type: "scheduled",
      command: {
        id: "migrate",
        runnable: "migrate",
        kind: "jssg",
        operation: {
          kind: "jssg",
          transform: { name: "migrate", hash: built.hash },
          language: "typescript",
          target: { root: "apps/web" },
        },
      },
    });
    // Only the completion (which this fake bridge fills with the echoed
    // context) mentions the source or the root; the recorded command does not.
    expect(JSON.stringify(scheduled)).not.toContain("transform(root");
    expect(JSON.stringify(scheduled)).not.toContain(repo);

    // A replay needs neither the artifact nor the checkout it was built on.
    const executed: OperationRequest[] = [];
    const elsewhere = new BridgeExecutor({ bin: fakeBridge, cwd: repo });
    const second = await run(wf, {
      executor: {
        execute(request, signal) {
          executed.push(request);
          return elsewhere.execute(request, signal);
        },
      },
      history: MemoryHistoryStore.fromJSON(store.serialize()),
    });
    expect(second.replayed).toBe(true);
    expect(second.output).toEqual(first.output);
    expect(executed).toHaveLength(0);
  });

  it("fails a command whose artifact the executor does not hold, without spawning", async () => {
    const events = new CollectingSink();
    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: repo, events });
    const store = new MemoryHistoryStore();
    const error = await run(wf, { executor, history: store, events }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(OperationError);
    expect((error as OperationError).status).toBe("failed");
    expect((error as OperationError).detail).toEqual({
      message: expect.stringMatching(
        /no built artifact for jssg 'migrate' \(hash [0-9a-f]{12}\); load the workflow with loadWorkflow\(\)/u,
      ),
      details: { phase: "artifact" },
    });
    expect(events.events.filter((e) => e.type === "bridge.spawned")).toHaveLength(0);
    expect(store.toJSON().events.map((e) => e.type)).toEqual(["scheduled", "completed"]);
  });

  it("runs exec through the one-shot file protocol and refuses ai locally", async () => {
    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: repo });
    const inspect = exec({ name: "inspect", command: "true" });
    const result = await run(
      workflow(() => inspect()),
      { executor },
    );
    const echoed = JSON.parse((result.output as { stdout: string }).stdout) as {
      request: OperationRequest;
    };
    expect(echoed.request.operation).toEqual({ kind: "exec", command: "true" });
    const ai = await executor.execute({
      protocolVersion: 5,
      commandId: "ai",
      operation: { kind: "ai", prompt: "x" },
    });
    expect(ai.status).toBe("failed");
  });
});

describe("codemod-workflow argument parsing", () => {
  it("resolves every path and defaults the target and bridge", () => {
    const options = parseArgs(["fixtures/jssg/workflow.ts"]);
    expect(options.workflow).toBe(resolve("fixtures/jssg/workflow.ts"));
    expect(options.target).toBe(process.cwd());
    expect(options.bridge).toBe(
      process.env.CODEMOD_BRIDGE_BIN === undefined
        ? resolve(import.meta.dirname, "../../../target/debug/butterflow-execution-bridge")
        : resolve(process.env.CODEMOD_BRIDGE_BIN),
    );
    expect(parseArgs(["wf.ts", "--target", "repo", "--bridge", "bin/bridge"])).toEqual({
      workflow: resolve("wf.ts"),
      target: resolve("repo"),
      bridge: resolve("bin/bridge"),
    });
  });

  it("rejects missing workflow, unknown or removed options, and dangling values", () => {
    expect(() => parseArgs([])).toThrow(USAGE);
    expect(() => parseArgs(["--target", "x"])).toThrow(USAGE);
    expect(() => parseArgs(["wf.ts", "--nope", "x"])).toThrow(/unknown option: --nope/u);
    expect(() => parseArgs(["wf.ts", "--script-root", "x"])).toThrow(
      /unknown option: --script-root/u,
    );
    expect(() => parseArgs(["wf.ts", "--target"])).toThrow(/missing value for --target/u);
  });
});

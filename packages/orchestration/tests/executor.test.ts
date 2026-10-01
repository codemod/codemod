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
import { ABORT_SIGNALS, parseArgs, runWorkflowCli, USAGE, type CliHooks } from "../src/host/cli.ts";
import type { DashboardSession } from "../src/host/dashboard/index.ts";
import {
  BridgeExecutor,
  CollectingSink,
  MemoryHistoryStore,
  OperationError,
  shell,
  isSafeRelativePath,
  jssg,
  run,
  dynamic,
  type OperationRequest,
} from "../src/index.ts";
import { artifact, canListen, ref } from "./helpers.ts";

const fakeBridge = resolve(import.meta.dirname, "fixtures/fake-bridge.mjs");
/** `--dashboard` opens a loopback listener, which a sandbox may deny. */
const listenable = await canListen();

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
const wf = dynamic(() => migrate({ target: { root: "apps/web" } }));

interface EchoOutput {
  path: string;
  context: { targetRoot: string; artifact: { source: string } };
}

describe.skipIf(process.platform === "win32")("BridgeExecutor artifacts", () => {
  let repo: string;
  beforeEach(() => {
    chmodSync(fakeBridge, 0o755);
    repo = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-shell-")));
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

  it("runs shell through the one-shot file protocol", async () => {
    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: repo });
    const inspect = shell({ name: "inspect", command: "true" });
    const result = await run(
      dynamic(() => inspect()),
      { executor },
    );
    const echoed = JSON.parse((result.output as { stdout: string }).stdout) as {
      request: OperationRequest;
    };
    expect(echoed.request.operation).toEqual({ kind: "shell", command: "true" });
  });
});

describe("codemod-workflow argument parsing", () => {
  it("aborts the run on SIGINT, SIGTERM, and SIGHUP", () => {
    expect([...ABORT_SIGNALS].sort()).toEqual(["SIGHUP", "SIGINT", "SIGTERM"]);
  });

  it("resolves every path and defaults the target and bridge", () => {
    const options = parseArgs(["fixtures/jssg/dynamic.ts"]);
    expect(options.workflow).toBe(resolve("fixtures/jssg/dynamic.ts"));
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
      dashboard: false,
    });
    expect(options).not.toHaveProperty("input");
  });

  it("treats --dashboard as a value-less opt-in flag", () => {
    expect(parseArgs(["wf.ts", "--dashboard"]).dashboard).toBe(true);
    expect(parseArgs(["wf.ts", "--dashboard", "--target", "repo"])).toMatchObject({
      dashboard: true,
      target: resolve("repo"),
    });
    expect(parseArgs(["wf.ts", "--target", "repo", "--dashboard"]).dashboard).toBe(true);
    expect(USAGE).toContain("[--dashboard]");
  });

  it("parses --input as strict JSON and keeps an explicit null apart from no input", () => {
    expect(parseArgs(["wf.ts", "--input", '{"replacement":"newApi"}']).input).toEqual({
      replacement: "newApi",
    });
    expect(parseArgs(["wf.ts", "--input", "null"])).toHaveProperty("input", null);
    expect(parseArgs(["wf.ts", "--input", "0"])).toHaveProperty("input", 0);
    expect(parseArgs(["wf.ts", "--input", '"text"'])).toHaveProperty("input", "text");
    expect(parseArgs(["wf.ts", "--input", "[1, 2]"]).input).toEqual([1, 2]);
    expect(parseArgs(["wf.ts", "--target", "repo"])).not.toHaveProperty("input");
    for (const malformed of ["", "{replacement: newApi}", "{'a': 1}", '{"a": 1,}', "undefined"]) {
      expect(() => parseArgs(["wf.ts", "--input", malformed]), malformed).toThrow(
        /--input must be valid JSON/u,
      );
    }
    expect(() => parseArgs(["wf.ts", "--input"])).toThrow(/missing value for --input/u);
  });

  it("rejects missing workflow, unknown or removed options, and dangling values", () => {
    expect(() => parseArgs([])).toThrow(USAGE);
    expect(() => parseArgs(["--target", "x"])).toThrow(USAGE);
    expect(() => parseArgs(["wf.ts", "--nope", "x"])).toThrow(/unknown option: --nope/u);
    expect(() => parseArgs(["wf.ts", "--script-root", "x"])).toThrow(
      /unknown option: --script-root/u,
    );
    expect(() => parseArgs(["wf.ts", "--target"])).toThrow(/missing value for --target/u);
    expect(USAGE).toContain("[--input <json>]");
  });
});

describe.skipIf(process.platform === "win32")("codemod-workflow runs", () => {
  const load = (file: string) => resolve(import.meta.dirname, "fixtures/load", file);
  const base = (file: string) => [
    load(file),
    "--target",
    import.meta.dirname,
    "--bridge",
    fakeBridge,
  ];
  beforeEach(() => chmodSync(fakeBridge, 0o755));

  it("runs a bare shell step as the root", async () => {
    const output = (await runWorkflowCli(base("shell.ts"))) as { stdout: string };
    const echoed = JSON.parse(output.stdout) as { request: OperationRequest };
    expect(echoed.request.commandId).toBe("inspect");
    expect(echoed.request.operation).toEqual({ kind: "shell", command: "true" });
  });

  /**
   * Seams into a `--dashboard` host: the session as soon as it exists, a
   * promise for the first run's creation, and (when asked) a server stand-in
   * so the test needs no socket.
   */
  function dashboardHooks(fakeServer: boolean) {
    let session: DashboardSession | undefined;
    let closed = 0;
    const hooks: CliHooks = {};
    const firstRun = new Promise<string>((resolve) => {
      hooks.onSession = (s) => {
        session = s;
        s.subscribe((notice) => {
          if (notice.type === "run.created") resolve(notice.run.runId);
        });
      };
    });
    if (fakeServer) {
      hooks.serve = async () => ({
        url: "http://127.0.0.1:0/",
        port: 0,
        close: async () => {
          closed += 1;
        },
      });
    }
    return {
      hooks,
      firstRun,
      session: () => session!,
      closed: () => closed,
    };
  }

  it.skipIf(!listenable)(
    "announces the dashboard out of band and still returns the result",
    async () => {
      const lines: string[] = [];
      const controller = new AbortController();
      const seams = dashboardHooks(false);
      const finished = runWorkflowCli(
        [...base("shell.ts"), "--dashboard"],
        controller.signal,
        (line) => lines.push(line),
        seams.hooks,
      );
      const firstRun = await seams.firstRun;
      await seams.session().settled(firstRun);
      controller.abort();
      const output = (await finished) as { stdout: string };
      expect(lines[0]).toMatch(/^dashboard: http:\/\/127\.0\.0\.1:\d+\/$/u);
      const echoed = JSON.parse(output.stdout) as { request: OperationRequest };
      expect(echoed.request.commandId).toBe("inspect");
    },
  );

  it("keeps the dashboard host alive after a run and exits cleanly on the signal", async () => {
    const lines: string[] = [];
    const controller = new AbortController();
    const seams = dashboardHooks(true);
    const finished = runWorkflowCli(
      [...base("shell.ts"), "--dashboard"],
      controller.signal,
      (line) => lines.push(line),
      seams.hooks,
    );
    let settled = false;
    void finished.then(
      () => {
        settled = true;
      },
      () => {
        settled = true;
      },
    );

    // The first run starts on its own and finishes, and the host stays up.
    const first = await seams.firstRun;
    const session = seams.session();
    await session.settled(first);
    expect(session.run(first)).toMatchObject({ number: 1, status: "completed" });
    await new Promise((resolve) => setImmediate(resolve));
    expect(settled).toBe(false);
    expect(session.isClosed).toBe(false);
    expect(seams.closed()).toBe(0);

    // Run again works while hosting, with a fresh run id.
    const second = await session.start();
    expect(second.runId).not.toBe(first);
    await session.settled(second.runId);
    expect(session.runs().map((r) => r.status)).toEqual(["completed", "completed"]);

    // Ctrl-C: the session closes, the server closes, and the newest result is the answer.
    controller.abort();
    const output = (await finished) as { stdout: string };
    expect(session.isClosed).toBe(true);
    expect(seams.closed()).toBe(1);
    expect(JSON.parse(output.stdout).request.commandId).toBe("inspect");
    expect(lines).toEqual([
      "dashboard: http://127.0.0.1:0/",
      "run 1 started",
      expect.stringMatching(/^run 1 done in \d+\.\ds$/u),
      "run 2 started",
      expect.stringMatching(/^run 2 done in \d+\.\ds$/u),
    ]);
  });

  it("fails like a plain run when the newest run did not complete", async () => {
    const lines: string[] = [];
    const controller = new AbortController();
    const seams = dashboardHooks(true);
    const finished = runWorkflowCli(
      [...base("shell.ts"), "--dashboard"],
      controller.signal,
      (line) => lines.push(line),
      seams.hooks,
    );
    const first = await seams.firstRun;
    const session = seams.session();
    await session.settled(first);
    // Stop the host while a second run is active: the abort lands before its
    // bridge process can answer, so that run is cancelled and reported as such.
    const second = await session.start();
    controller.abort();
    await expect(finished).rejects.toThrow(/cancelled|aborted/iu);
    expect(session.run(second.runId)).toMatchObject({ status: "cancelled" });
    expect(seams.closed()).toBe(1);
    expect(lines.at(-1)).toMatch(/^run 2 stopped after/u);
  });

  it("requires --input for a root that declares input, and passes null and JSON through", async () => {
    await expect(runWorkflowCli(base("input.ts"))).rejects.toThrow(
      /workflow requires an input value; pass --input <json>/u,
    );
    expect(await runWorkflowCli([...base("input.ts"), "--input", "null"])).toEqual({
      received: null,
    });
    expect(await runWorkflowCli([...base("input.ts"), "--input", '{"name":"x"}'])).toEqual({
      received: { name: "x" },
    });
    await expect(runWorkflowCli([...base("input.ts"), "--input", "{name: x}"])).rejects.toThrow(
      /--input must be valid JSON/u,
    );
  });
});

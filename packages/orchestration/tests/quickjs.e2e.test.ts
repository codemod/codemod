/**
 * End-to-end tests of `--runtime=quickjs` against the real Rust host
 * (`crates/orchestration-quickjs-host`).
 *
 * Run with: pnpm --filter @codemod.com/orchestration test:e2e
 * (builds the host). Override it with CODEMOD_QUICKJS_HOST_BIN.
 */
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { buildFile, scheduledCommands, type History } from "../src/index.ts";
import { buildQuickjsBundle } from "../src/host/quickjs-bundle.ts";

const enabled = process.env.CODEMOD_QUICKJS_E2E === "1";
const repoRoot = resolve(import.meta.dirname, "../../..");
const host =
  process.env.CODEMOD_QUICKJS_HOST_BIN ?? join(repoRoot, "target/debug/orchestration-quickjs-host");
const fixtures = resolve(import.meta.dirname, "fixtures/quickjs");
const fakeAgent = resolve(import.meta.dirname, "fixtures/fake-agent.mjs");
const scratch = mkdtempSync(join(tmpdir(), "quickjs-e2e-"));

beforeAll(() => {
  if (!enabled) return;
  if (!existsSync(host)) throw new Error(`quickjs host binary not found at ${host}`);
  process.env.CODEMOD_QUICKJS_HOST_BIN = host;
  process.env.CODEMOD_WORKFLOW_CACHE = join(scratch, "cache");
});
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

let counter = 0;
const FILES: Record<string, string> = {
  "a.ts": "oldApi(1);\n",
  "b.ts": "export const b = 1;\n",
  "c.ts": "export const c = oldApi(2);\n",
  "sub/d.old.ts": "export const d = 3;\n",
};

function makeRepo(extra: Record<string, string> = {}): string {
  const dir = join(scratch, `repo-${++counter}`);
  for (const [path, content] of Object.entries({ ...FILES, ...extra })) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
  return dir;
}

function snapshot(dir: string): Record<string, string> {
  const files: Record<string, string> = {};
  for (const name of readdirSync(dir, { recursive: true, encoding: "utf8" })) {
    const path = join(dir, name);
    if (readdirSafe(path)) continue;
    files[relative(dir, path).split("\\").join("/")] = readFileSync(path, "utf8");
  }
  return files;
}
const readdirSafe = (path: string) => {
  try {
    readdirSync(path);
    return true;
  } catch {
    return false;
  }
};

type Outcome =
  | { ok: true; output: unknown; files: Record<string, string> }
  | {
      ok: false;
      error: string;
      files: Record<string, string>;
    };

const cli = resolve(import.meta.dirname, "../bin/codemod-workflow.mjs");

/** `codemod-workflow run --runtime=quickjs` in a subprocess: the real CLI path. */
function runOn(
  fixture: string,
  input?: unknown,
  options: { files?: Record<string, string>; env?: Record<string, string> } = {},
): Outcome {
  const target = makeRepo(options.files);
  const args = [cli, "run", "--runtime=quickjs", join(fixtures, fixture), "--target", target];
  if (input !== undefined) args.push("--input", JSON.stringify(input));
  const result = spawnSync(process.execPath, args, {
    encoding: "utf8",
    env: { ...process.env, ...options.env },
  });
  if (result.status === 0) {
    return { ok: true, output: JSON.parse(result.stdout) as unknown, files: snapshot(target) };
  }
  return { ok: false, error: result.stderr.trim(), files: snapshot(target) };
}

function runHost(args: string[]): {
  status: number | null;
  json: Record<string, unknown>;
  stderr: string;
} {
  const result = spawnSync(host, args, { encoding: "utf8" });
  return {
    status: result.status,
    json: JSON.parse(result.stdout || "null") as Record<string, unknown>,
    stderr: result.stderr,
  };
}

async function bundleFor(fixture: string): Promise<string> {
  const bundle = await buildQuickjsBundle(join(fixtures, fixture));
  const path = join(scratch, `${bundle.hash}.mjs`);
  writeFileSync(path, bundle.code);
  return path;
}

/** `FILES` after the fixtures' `replace` and `rename` transforms. */
const MIGRATED = {
  "a.ts": "newApi(1);\n",
  "b.ts": "export const b = 1;\n",
  "c.ts": "export const c = newApi(2);\n",
  "sub/d.new.ts": "export const d = 3;\n",
};

describe.runIf(enabled)("quickjs runtime: JSSG commands", () => {
  it("runs a sequence: structured outputs, selector skips, renames", () => {
    expect(runOn("jssg-sequence.ts")).toEqual({ ok: true, output: [], files: MIGRATED });
  });

  it("runs dynamic bodies, concurrent commands, invocation targets, and a shell step", () => {
    expect(runOn("jssg-dynamic.ts")).toEqual({
      ok: true,
      output: {
        first: [{ file: "a.ts" }],
        second: [],
        third: [],
        stdout: "./a.ts\n./b.ts\n./c.ts\n./sub/d.new.ts\nnewApi(1);\n",
      },
      files: MIGRATED,
    });
  });

  it("splits every module in the workflow's graph into artifacts", () => {
    const workflow = resolve(import.meta.dirname, "fixtures/load/dynamic.ts");
    const built = buildFile(resolve(import.meta.dirname, "fixtures/load/definitions.ts"));
    const run = runOn(relative(fixtures, workflow));
    expect(run.ok, run.ok ? "" : run.error).toBe(true);
    expect((run as { output: { transform: unknown } }).output.transform).toEqual({
      name: "migrate",
      hash: built.artifacts[0]!.hash,
    });
    expect(run.files["a.ts"]).toBe("newApi(1);\n");
  });

  it.each([
    ["fail-second", /second file exploded/u],
    ["conflict", /same\.ts/u],
    ["escape", /is outside the target directory/u],
  ])("failure '%s' fails the run and writes nothing", (which, message) => {
    const outcome = runOn("jssg-failures.ts", which);
    expect(outcome.ok).toBe(false);
    expect(outcome.ok === false && outcome.error).toMatch(message);
    expect(outcome.files).toEqual(FILES);
  });
});

describe.runIf(enabled)("quickjs runtime: root input", () => {
  const echo = "../load/input.ts";

  it("requires --input for a root that declares input", () => {
    expect(runOn(echo)).toMatchObject({
      ok: false,
      error: expect.stringMatching(/workflow requires an input value; pass --input <json>/u),
    });
  });

  it.each([
    ["null", null],
    ['{"name":"x"}', { name: "x" }],
  ])("passes --input %s through", (text, value) => {
    expect(runOn(echo, JSON.parse(text))).toMatchObject({ ok: true, output: { received: value } });
  });
});

describe.runIf(enabled)("quickjs runtime: native file selection", () => {
  // Ignore files at several levels, a negation, a parent rule a negation
  // cannot undo, and a nested `.ignore`, around the parity fixtures' files.
  const files = {
    ".gitignore": "*.gen.ts\nvendor/\n!keep.gen.ts\n",
    "keep.gen.ts": "oldApi(5);\n",
    "x.gen.ts": "oldApi(6);\n",
    "vendor/v.ts": "oldApi(7);\n",
    "sub/.ignore": "!*.gen.ts\n",
    "sub/y.gen.ts": "oldApi(8);\n",
    "sub/deeper/.gitignore": "z.ts\n",
    "sub/deeper/z.ts": "oldApi(9);\n",
    "sub/deeper/w.ts": "oldApi(10);\n",
  };

  it.each(["jssg-sequence.ts", "jssg-dynamic.ts"])(
    "%s: the same run with the walker interpreted",
    (fixture) => {
      const native = runOn(fixture, undefined, { files });
      const interpreted = runOn(fixture, undefined, {
        files,
        env: { CODEMOD_NATIVE_SELECT: "0" },
      });
      expect(native.ok).toBe(true);
      expect(interpreted).toEqual(native);
    },
  );
});

describe.runIf(enabled)("quickjs runtime: control plane inside QuickJS", () => {
  it("resumes a failed run from its history without re-executing completed commands", async () => {
    const bundle = await bundleFor("shell-resume.ts");
    const target = makeRepo();
    const history = join(target, "..", `history-${counter}.json`);
    const args = ["run", bundle, "--target", target, "--history", history, "--json"];
    const log = () => readFileSync(join(target, "runs.log"), "utf8");

    const first = runHost([...args, "--input", "true"]);
    expect(first.status).toBe(1);
    expect(first.json).toEqual({ ok: false, error: "crashed before gate" });
    expect(log()).toBe("count\n");

    const second = runHost([...args, "--input", "false"]);
    expect(second.json).toEqual({ ok: true, output: "passed\n" });
    expect(log()).toBe("count\ngate\n");

    const recorded = JSON.parse(readFileSync(history, "utf8")) as History;
    expect(scheduledCommands(recorded).map((command) => command.id)).toEqual(["count", "gate"]);
    expect(recorded.events.filter((event) => event.type === "finalized")).toHaveLength(1);
    // A third run only replays.
    expect(runHost([...args, "--input", "false"]).json).toEqual(second.json);
    expect(log()).toBe("count\ngate\n");
  });

  it("propagates the run context like AsyncLocalStorage", async () => {
    const bundle = await bundleFor("parallel-dynamic.ts");
    const target = makeRepo();
    const history = join(scratch, `history-parallel-${counter}.json`);
    const quickjs = runHost(["run", bundle, "--target", target, "--history", history, "--json"]);
    expect(quickjs.json).toEqual({ ok: true, output: ["a2\n", ["b1\n", "c1\n", "b2\n"]] });
    const recorded = scheduledCommands(JSON.parse(readFileSync(history, "utf8")) as History);
    expect(recorded.map((command) => command.id).sort()).toEqual(["a1", "a2", "b1", "b2", "c1"]);
    // Every command is issued from a parallel member's body, after awaits
    // or inside a nested sequence, so all are concurrent, as with
    // AsyncLocalStorage (the Node test harness records the same). Without
    // propagation they would lose the flag.
    expect(recorded.filter((command) => command.concurrent === true)).toHaveLength(5);
  });

  it("runs JSSG semantic analysis (workspace and file scope)", () => {
    const files = {
      "main.ts": 'import { add } from "./utils";\nconst result = add(1, 2);\n',
      "utils.ts": "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
    };
    const quickjs = runOn("semantic.ts", undefined, { files });
    expect(quickjs.ok, quickjs.ok ? "" : quickjs.error).toBe(true);
    const [workspace, file] = (quickjs as { output: [unknown, { file: string; kind: unknown }[]] })
      .output;
    expect(workspace).toEqual([{ file: "main.ts", definition: "utils.ts" }]);
    expect(quickjs.files["utils.ts"]).toContain("function sum");
    expect(file.find((entry) => entry.file === "main.ts")).toMatchObject({ kind: "local" });
  });

  it("runs agent steps through codemod-agent with the allowlisted environment", async () => {
    const run = runOn("static-agent.ts", undefined, {
      env: { CODEMOD_AGENT_BIN: fakeAgent, LLM_API_KEY: "llm-key", TYPESAFE_API_KEY: "ts-key" },
    });
    expect(run.ok, run.ok ? "" : run.error).toBe(true);
    expect(run.files["ran.marker"]).toBe("");
    const echo = JSON.parse((run as { output: { text: string } }).output.text) as {
      request: { commandId: string; operation: { kind: string } };
      env: Record<string, string>;
      secrets: unknown;
    };
    expect(echo.request).toMatchObject({ commandId: "fix", operation: { kind: "agent" } });
    expect(echo.secrets).toEqual({ LLM_API_KEY: "llm-key" });
    expect(echo.env).toMatchObject({ CODEMOD_AGENT_SECRETS: "stdin" });
    expect(echo.env).not.toHaveProperty("LLM_API_KEY");
    expect(echo.env).not.toHaveProperty("TYPESAFE_API_KEY");

    const missing = runOn("static-agent.ts", undefined, {
      env: { CODEMOD_AGENT_BIN: join(scratch, "no-such-agent") },
    });
    expect(missing.ok === false && missing.error).toMatch(
      /agent steps need the codemod-agent runner/u,
    );
  });

  it("SIGTERM during an agent step kills the runner's whole process tree", async () => {
    const bundle = await bundleFor("static-agent.ts");
    const target = makeRepo();
    const pidFile = join(target, "pids.json");
    writeFileSync(
      join(target, "fake-agent.json"),
      JSON.stringify({
        FAKE_AGENT_MODE: "hang",
        FAKE_PID_FILE: pidFile,
        FAKE_CHILDREN: "2",
        FAKE_DETACH_CHILDREN: "1",
      }),
    );
    const child = spawn(host, ["run", bundle, "--target", target, "--json"], {
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, CODEMOD_AGENT_BIN: fakeAgent },
    });
    let stdout = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    const deadline = Date.now() + 10_000;
    while (!existsSync(pidFile) || readFileSync(pidFile, "utf8").trim() === "") {
      if (Date.now() > deadline) throw new Error("the agent never started");
      await new Promise((r) => setTimeout(r, 20));
    }
    const pids = JSON.parse(readFileSync(pidFile, "utf8")) as number[];
    child.kill("SIGTERM");
    const code = await new Promise<number | null>((r) => child.once("close", r));
    expect(code).toBe(1);
    expect(JSON.parse(stdout)).toMatchObject({ ok: false });
    const alive = (pid: number) => {
      try {
        process.kill(pid, 0);
        return true;
      } catch {
        return false;
      }
    };
    const until = Date.now() + 5_000;
    while (pids.some(alive) && Date.now() < until) await new Promise((r) => setTimeout(r, 20));
    expect(pids.filter(alive)).toEqual([]);
  });

  it("runs assessment through the host's httpRequest, only to the TypeSafe API root", async () => {
    const log = join(scratch, "typesafe-requests.jsonl");
    writeFileSync(log, "");
    const stub = spawn(
      process.execPath,
      [join(import.meta.dirname, "fixtures/typesafe-stub.mjs"), log],
      {
        stdio: ["ignore", "pipe", "inherit"],
      },
    );
    try {
      const port = await new Promise<number>((resolvePort, reject) => {
        stub.stdout!.setEncoding("utf8");
        stub.stdout!.once("data", (line: string) => resolvePort(Number(line.trim())));
        stub.once("error", reject);
      });
      const env = { TYPESAFE_API_KEY: "test-key", TYPESAFE_BASE_URL: `http://127.0.0.1:${port}` };
      const expected = ["a.ts", "b.ts", "c.ts", "sub/d.old.ts"].map((file) => ({
        file,
        assessment: {
          model: "stub-1",
          answers: { risky: { type: "noul", noul: 0.25 } },
          usage: { inputTokens: 3, outputTokens: 1 },
        },
      }));
      const quickjs = runOn("assessment.ts", undefined, { env });
      expect(quickjs).toMatchObject({ ok: true });
      expect(quickjs.ok && quickjs.output).toEqual(expected);
      const requests = readFileSync(log, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line) as { path: string; authorization: string });
      expect(requests).toHaveLength(4);
      expect(new Set(requests.map((request) => request.path))).toEqual(new Set(["/v1/systemone"]));
      expect(requests.every((request) => request.authorization === "Bearer test-key")).toBe(true);
      // Workflow code calling the primitive directly: only the API root.
      writeFileSync(log, "");
      const scoped = { ...env, TYPESAFE_BASE_URL: `http://127.0.0.1:${port}/v1` };
      for (const [url, code] of [
        [`http://127.0.0.1:${port}/other`, "EACCES"],
        [`http://localhost:${port}/v1/systemone`, "EACCES"],
        [`https://example.com/v1/systemone`, "EACCES"],
        [`http://127.0.0.1:${port}/v1/models`, "reached"],
      ] as const) {
        expect(runOn("http-escape.ts", url, { env: scoped })).toMatchObject({
          ok: true,
          output: code,
        });
      }
      expect(readFileSync(log, "utf8").trim().split("\n")).toHaveLength(1);
    } finally {
      stub.kill();
    }
  });

  it("SIGTERM aborts the run and kills the running shell's process group", async () => {
    const bundle = await bundleFor("shell-hang.ts");
    const target = makeRepo();
    const child = spawn(host, ["run", bundle, "--target", target, "--json"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    const pidFile = join(target, "hang.pid");
    const deadline = Date.now() + 10_000;
    while (!existsSync(pidFile) || readFileSync(pidFile, "utf8").trim() === "") {
      if (Date.now() > deadline) throw new Error("the shell step never started");
      await new Promise((r) => setTimeout(r, 20));
    }
    const pid = Number(readFileSync(pidFile, "utf8").trim());
    const started = Date.now();
    child.kill("SIGTERM");
    const code = await new Promise<number | null>((r) => child.once("close", r));
    expect(Date.now() - started).toBeLessThan(5_000);
    expect(code).toBe(1);
    expect(JSON.parse(stdout)).toMatchObject({ ok: false });
    expect(() => process.kill(pid, 0)).toThrow();
  });

  it("SIGINT cancels a running JSSG batch promptly and writes nothing", async () => {
    const bundle = await bundleFor("jssg-long.ts");
    // 300 files x ~10 ms: about 3 s of transforms if the signal were ignored.
    const many = Object.fromEntries(
      Array.from({ length: 300 }, (_, i) => [`many/f${i}.ts`, `oldApi(${i});\n`]),
    );
    const target = makeRepo(many);
    const before = snapshot(target);
    const child = spawn(host, ["run", bundle, "--target", target, "--json"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    const marker = join(target, "started.marker");
    const deadline = Date.now() + 10_000;
    while (!existsSync(marker)) {
      if (Date.now() > deadline) throw new Error("the workflow never started");
      await new Promise((r) => setTimeout(r, 10));
    }
    // Well inside the batch (the shell step is done once the marker exists).
    await new Promise((r) => setTimeout(r, 300));
    const signalled = Date.now();
    child.kill("SIGINT");
    const code = await new Promise<number | null>((r) => child.once("close", r));
    // At most one 64-file chunk (~0.65 s) plus the file in flight.
    expect(Date.now() - signalled).toBeLessThan(1_500);
    expect(code).toBe(1);
    expect(JSON.parse(stdout)).toMatchObject({
      ok: false,
      error: expect.stringMatching(/command 'slow' cancelled/u) as unknown,
    });
    const after = snapshot(target);
    delete after["started.marker"];
    expect(after).toEqual(before);
  });
});

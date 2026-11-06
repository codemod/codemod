/**
 * End-to-end tests of `--runtime=quickjs` against the real Rust host
 * (`crates/orchestration-quickjs-host`) and, for parity, the real bridge.
 *
 * Run with: pnpm --filter @codemod.com/orchestration test:e2e
 * (builds both binaries). Override them with CODEMOD_QUICKJS_HOST_BIN and
 * CODEMOD_BRIDGE_BIN.
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
import { scheduledCommands, type History } from "../src/index.ts";
import { buildQuickjsBundle } from "../src/host/quickjs-bundle.ts";

const enabled = process.env.CODEMOD_QUICKJS_E2E === "1";
const repoRoot = resolve(import.meta.dirname, "../../..");
const host =
  process.env.CODEMOD_QUICKJS_HOST_BIN ?? join(repoRoot, "target/debug/orchestration-quickjs-host");
const bridge =
  process.env.CODEMOD_BRIDGE_BIN ?? join(repoRoot, "target/debug/butterflow-execution-bridge");
const fixtures = resolve(import.meta.dirname, "fixtures/quickjs");
const scratch = mkdtempSync(join(tmpdir(), "quickjs-e2e-"));

beforeAll(() => {
  if (!enabled) return;
  for (const [name, path] of [
    ["quickjs host", host],
    ["bridge", bridge],
  ]) {
    if (!existsSync(path!)) throw new Error(`${name} binary not found at ${path}`);
  }
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

function makeRepo(): string {
  const dir = join(scratch, `repo-${++counter}`);
  for (const [path, content] of Object.entries(FILES)) {
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
const register = resolve(import.meta.dirname, "../bin/register-ts.mjs");

/**
 * `codemod-workflow run --runtime=<runtime>` in a subprocess: the real CLI
 * path (vitest's module loader would bypass `loadWorkflow`'s Node hooks).
 */
function runOn(runtime: "bridge" | "quickjs", fixture: string, input?: unknown): Outcome {
  const target = makeRepo();
  const args = [cli, "run", `--runtime=${runtime}`, join(fixtures, fixture), "--target", target];
  args.push("--bridge", bridge);
  if (input !== undefined) args.push("--input", JSON.stringify(input));
  const result = spawnSync(process.execPath, args, { encoding: "utf8", env: process.env });
  if (result.status === 0) {
    return { ok: true, output: JSON.parse(result.stdout) as unknown, files: snapshot(target) };
  }
  return { ok: false, error: result.stderr.trim(), files: snapshot(target) };
}

/** The workflow run in Node (AsyncLocalStorage) by `fixtures/quickjs/node-reference.ts`. */
function nodeReference(fixture: string): { output: unknown; history: History } {
  const result = spawnSync(
    process.execPath,
    ["--import", register, join(fixtures, "node-reference.ts"), join(fixtures, fixture)],
    { encoding: "utf8" },
  );
  if (result.status !== 0) throw new Error(`node reference failed: ${result.stderr}`);
  return JSON.parse(result.stdout) as { output: unknown; history: History };
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

describe.runIf(enabled)("quickjs runtime vs bridge runtime: JSSG parity", () => {
  it("a sequence: structured outputs, selector skips, renames", async () => {
    const [b, q] = [
      await runOn("bridge", "jssg-sequence.ts"),
      await runOn("quickjs", "jssg-sequence.ts"),
    ];
    expect(b.ok).toBe(true);
    expect(q).toEqual(b);
    expect(q.files["sub/d.new.ts"]).toBe("export const d = 3;\n");
    expect(q.files["a.ts"]).toBe("newApi(1);\n");
  });

  it("dynamic bodies, concurrent commands, invocation targets, and a shell step", async () => {
    const [b, q] = [
      await runOn("bridge", "jssg-dynamic.ts"),
      await runOn("quickjs", "jssg-dynamic.ts"),
    ];
    expect(b.ok).toBe(true);
    expect(q).toEqual(b);
  });

  it.each([
    ["fail-second", /second file exploded/u],
    ["conflict", /same\.ts/u],
    ["escape", /is outside the target directory/u],
  ])("failure '%s' fails on both and writes nothing", async (which, message) => {
    const [b, q] = [
      await runOn("bridge", "jssg-failures.ts", which),
      await runOn("quickjs", "jssg-failures.ts", which),
    ];
    for (const outcome of [b, q]) {
      expect(outcome.ok).toBe(false);
      expect(outcome.ok === false && outcome.error).toMatch(message);
      expect(outcome.files).toEqual(FILES);
    }
  });
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

  it("propagates the run context like AsyncLocalStorage: same history as a Node run", async () => {
    const bundle = await bundleFor("parallel-dynamic.ts");
    const target = makeRepo();
    const history = join(scratch, `history-parallel-${counter}.json`);
    const quickjs = runHost(["run", bundle, "--target", target, "--history", history, "--json"]);
    expect(quickjs.json.ok).toBe(true);

    // Reference: the same module in Node (AsyncLocalStorage).
    const node = nodeReference("parallel-dynamic.ts");
    expect(quickjs.json.output).toEqual(node.output);
    const byId = (history: History) =>
      scheduledCommands(history).sort((a, b) => (a.id < b.id ? -1 : 1));
    const recorded = byId(JSON.parse(readFileSync(history, "utf8")) as History);
    expect(recorded).toEqual(byId(node.history));
    // Commands issued after awaits in the parallel members' bodies are
    // concurrent; the nested dynamic's command is not (as on Node). With the
    // fallback context they would all lose the flag.
    expect(
      recorded
        .filter((command) => command.concurrent === true)
        .map((c) => c.id)
        .sort(),
    ).toEqual(["a1", "a2", "b1", "b2"]);
  });

  it("refuses agent and JSSG semantic analysis with a clear error", async () => {
    const agentRun = await runOn("quickjs", "unsupported.ts", "agent");
    expect(agentRun.ok === false && agentRun.error).toMatch(
      /operation kind 'agent' is unsupported in runtime 'quickjs'/u,
    );
    const semanticRun = await runOn("quickjs", "unsupported.ts", "semantic");
    expect(semanticRun.ok === false && semanticRun.error).toMatch(
      /JSSG semantic analysis is unsupported in runtime 'quickjs'/u,
    );
    const staticRun = await runOn("quickjs", "static-agent.ts");
    expect(staticRun.ok === false && staticRun.error).toMatch(
      /operation kind 'agent' is unsupported in runtime 'quickjs' \(command 'fix'\)/u,
    );
    expect(staticRun.files["ran.marker"]).toBeUndefined();
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
});

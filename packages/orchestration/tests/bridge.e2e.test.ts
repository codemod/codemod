/**
 * Cross-language tests against the real Rust binary
 * (`butterflow-execution-bridge`): `exec` through the one-shot file protocol
 * and butterflow_runners::DirectRunner, and JSSG through the TypeScript
 * orchestrator over one persistent `--jssg-worker` process.
 *
 * Run with: pnpm --filter @codemod.com/orchestration test:e2e
 * (builds only crates/execution-bridge, then runs this file).
 * Override the binary with CODEMOD_BRIDGE_BIN=/path/to/butterflow-execution-bridge.
 */
import {
  cpSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
  BridgeExecutor,
  CollectingSink,
  MemoryHistoryStore,
  OperationError,
  exec,
  guard,
  jssg,
  run,
  workflow,
  type OperationCompletion,
  type OperationExecutor,
  type OperationRequest,
  type Target,
} from "../src/index.ts";

const bin =
  process.env.CODEMOD_BRIDGE_BIN ??
  resolve(import.meta.dirname, "../../../target/debug/butterflow-execution-bridge");
const scripts = resolve(import.meta.dirname, "fixtures/jssg");

beforeAll(() => {
  if (!existsSync(bin)) {
    throw new Error(
      `bridge binary not found at ${bin}; run 'cargo build -p butterflow-execution-bridge' or set CODEMOD_BRIDGE_BIN`,
    );
  }
});

const Runs = guard(
  "Runs",
  (v: unknown): v is { runs: number } => typeof v === "object" && v !== null,
);
const touch = exec({
  name: "touch",
  command: `echo run >> marker.txt && printf '{"runs":%s}' "$(grep -c run marker.txt)"`,
  output: Runs,
});
const failing = exec({ name: "failing", command: "echo boom >&2; exit 3" });
const envEcho = exec({
  name: "env",
  command: "printf '%s' \"$BRIDGE_TEST\"",
  env: { BRIDGE_TEST: "from-request" },
});

const wf = workflow(async () => {
  const first = await touch();
  const env = await envEcho();
  let failure = "";
  try {
    await failing();
  } catch (error) {
    if (!(error instanceof OperationError)) throw error;
    failure = `${error.status}:${error.detail?.exitCode ?? "none"}`;
  }
  return { runs: first.runs, env: env.stdout, failure };
});

describe("execution bridge end-to-end", () => {
  let dir: string;
  let calls: OperationRequest[];
  let executor: OperationExecutor;

  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), "codemod-bridge-"));
    calls = [];
    const bridge = new BridgeExecutor({ bin: relative(process.cwd(), bin), cwd: dir });
    executor = {
      execute(request, signal) {
        calls.push(request);
        return bridge.execute(request, signal);
      },
    };
  });

  afterAll(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  it("executes through DirectRunner, then replays from history without re-executing", async () => {
    const store = new MemoryHistoryStore();
    const first = await run(wf, { executor, history: store });

    // DirectRunner collects output line by line, so stdout always ends with "\n".
    expect(first.output).toEqual({ runs: 1, env: "from-request\n", failure: "failed:3" });
    expect(readFileSync(join(dir, "marker.txt"), "utf8")).toBe("run\n");
    expect(calls.map((c) => c.commandId)).toEqual(["touch", "env", "failing"]);
    const failedCompletion = first.history.events.find(
      (e) => e.type === "completed" && e.commandId === "failing",
    );
    expect(failedCompletion).toMatchObject({
      completion: { status: "failed", error: { exitCode: 3, output: "boom\n" } },
    });

    const reloaded = MemoryHistoryStore.fromJSON(store.serialize());
    const second = await run(wf, { executor, history: reloaded });

    expect(second.replayed).toBe(true);
    expect(second.output).toEqual(first.output);
    expect(calls).toHaveLength(3);
    expect(readFileSync(join(dir, "marker.txt"), "utf8")).toBe("run\n");
  });
});

/** A repository with files inside and outside the fixture workflow's target. */
function seedRepository(): string {
  const target = mkdtempSync(join(tmpdir(), "codemod-jssg-"));
  mkdirSync(join(target, "src"));
  mkdirSync(join(target, "other"));
  writeFileSync(join(target, "src", "b.ts"), "oldApi('b');\n");
  writeFileSync(join(target, "src", "a.ts"), "oldApi('a');\n");
  writeFileSync(join(target, "src", "skip.generated.ts"), "oldApi('skip');\n");
  writeFileSync(join(target, "other", "outside.ts"), "oldApi('outside');\n");
  return target;
}

function expectMigrated(target: string, stdout: string): void {
  expect(JSON.parse(stdout)).toEqual([{ file: "src/a.ts" }, { file: "src/b.ts" }]);
  expect(readFileSync(join(target, "src", "a.ts"), "utf8")).toBe("newApi('a');\n");
  expect(readFileSync(join(target, "src", "b.ts"), "utf8")).toBe("newApi('b');\n");
  expect(readFileSync(join(target, "src", "skip.generated.ts"), "utf8")).toBe("oldApi('skip');\n");
  expect(readFileSync(join(target, "other", "outside.ts"), "utf8")).toBe("oldApi('outside');\n");
}

describe("local TypeScript JSSG workflow end-to-end", () => {
  it("intersects targets, writes edits, aggregates output, and runs through the CLI", () => {
    const target = seedRepository();
    // The fixture's `script: "transform.ts"` resolves against the workflow's
    // directory, which is the CLI's default script root.
    const workflowPath = resolve(import.meta.dirname, "fixtures/jssg/workflow.ts");

    try {
      const result = spawnSync(
        process.execPath,
        [
          resolve(import.meta.dirname, "../bin/codemod-workflow.mjs"),
          workflowPath,
          "--target",
          target,
          "--bridge",
          bin,
        ],
        { encoding: "utf8" },
      );
      expect(result.status, result.stderr).toBe(0);
      expectMigrated(target, result.stdout);
    } finally {
      rmSync(target, { recursive: true, force: true });
    }
  });

  it("runs from a package installed under node_modules", () => {
    // Copy (not link) the package into a consumer's node_modules so its `.ts`
    // sources sit under a real node_modules path, which Node's built-in type
    // stripping refuses; the bin's loader hook must cover them.
    const consumer = mkdtempSync(join(tmpdir(), "codemod-consumer-"));
    const installed = join(consumer, "node_modules", "@codemod.com", "orchestration");
    const packageDir = resolve(import.meta.dirname, "..");
    for (const entry of ["package.json", "bin", "src"]) {
      cpSync(join(packageDir, entry), join(installed, entry), { recursive: true });
    }
    writeFileSync(join(consumer, "package.json"), JSON.stringify({ type: "module" }));
    mkdirSync(join(consumer, "scripts"));
    cpSync(
      resolve(import.meta.dirname, "fixtures/jssg/transform.ts"),
      join(consumer, "scripts", "migrate.ts"),
    );
    writeFileSync(
      join(consumer, "workflow.ts"),
      `import { jssg, workflow } from "@codemod.com/orchestration";
const migrate = jssg<void, { file: string }[]>({
  name: "migrate",
  script: "scripts/migrate.ts",
  language: "typescript",
  include: ["**/*.ts"],
});
export default workflow(() =>
  migrate({ target: { include: ["src/**"], exclude: ["**/*.generated.ts"] } }),
);
`,
    );
    const target = seedRepository();

    try {
      const result = spawnSync(
        process.execPath,
        [
          join(installed, "bin", "codemod-workflow.mjs"),
          "workflow.ts",
          "--target",
          target,
          "--bridge",
          bin,
        ],
        { cwd: consumer, encoding: "utf8" },
      );
      expect(result.status, result.stderr).toBe(0);
      expectMigrated(target, result.stdout);
    } finally {
      rmSync(target, { recursive: true, force: true });
      rmSync(consumer, { recursive: true, force: true });
    }
  });
});

describe("TypeScript JSSG orchestration over one persistent Rust worker", () => {
  let repo: string;
  const write = (relativePath: string, content: string) => {
    const path = join(repo, relativePath);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, content);
  };
  const read = (relativePath: string) => readFileSync(join(repo, relativePath), "utf8");

  beforeEach(() => {
    repo = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-worker-")));
  });
  afterEach(() => rmSync(repo, { recursive: true, force: true }));

  interface RunOutcome {
    completion: OperationCompletion;
    events: CollectingSink;
    output: unknown;
    error: unknown;
  }

  /** Run one JSSG command through `run()` and return its recorded completion. */
  async function runJssg(
    definition: Parameters<typeof jssg>[0],
    invocation: { target?: Target; id?: string } = {},
    options: { scriptRoot?: string; signal?: AbortSignal; history?: MemoryHistoryStore } = {},
  ): Promise<RunOutcome> {
    const events = new CollectingSink();
    const executor = new BridgeExecutor({
      bin,
      cwd: repo,
      scriptRoot: options.scriptRoot ?? scripts,
      events,
    });
    const command = jssg<void, unknown>(definition);
    const body = workflow(() => command(invocation));
    const history = options.history ?? new MemoryHistoryStore();
    let output: unknown;
    let error: unknown;
    try {
      ({ output } = await run(body, { executor, history, events, signal: options.signal }));
    } catch (caught) {
      error = caught;
    }
    const completed = (await history.load()).events.find((e) => e.type === "completed");
    if (!completed || completed.type !== "completed") throw new Error("no completion recorded");
    return { completion: completed.completion, events, output, error };
  }

  it("shares one session: workspace semantics across files, staged write() edits, outputs together", async () => {
    write("main.ts", 'import { add } from "./utils";\nconst result = add(1, 2);\n');
    write("utils.ts", "export function add(a: number, b: number): number {\n  return a + b;\n}\n");
    write("other.ts", "export const unrelated = 1;\n");

    const { completion, events } = await runJssg({
      name: "semantic",
      script: "semantic.ts",
      language: "typescript",
      semanticAnalysis: "workspace",
    });

    expect(completion.status, JSON.stringify(completion)).toBe("succeeded");
    expect(completion.output).toEqual([
      { file: "main.ts", definition: "utils.ts" },
      { file: "other.ts", definition: null },
      { file: "utils.ts", definition: null },
    ]);
    expect(read("utils.ts")).toContain("function sum");
    expect(read("main.ts")).toContain("add(1, 2)");
    expect(events.events.filter((e) => e.type === "jssg.worker")).toHaveLength(1);
    const phases = events.events.flatMap((e) => (e.type === "jssg.progress" ? [e.phase] : []));
    expect(phases.slice(0, 4)).toEqual(["select", "index", "index", "index"]);
    expect(phases.at(-1)).toBe("commit");
  });

  it("selects by the language's extensions from the worker when the definition has no include", async () => {
    write("a.ts", "oldApi('a');\n");
    write("b.js", "oldApi('b');\n");
    write("c.tsx", "oldApi('c');\n");
    write("d.md", "oldApi('d');\n");
    const { completion } = await runJssg({
      name: "t",
      script: "transform.ts",
      language: "typescript",
    });
    expect(completion.status).toBe("succeeded");
    expect(completion.output).toEqual([{ file: "a.ts" }, { file: "b.js" }]);
    expect(read("a.ts")).toBe("newApi('a');\n");
    expect(read("c.tsx")).toBe("oldApi('c');\n");
    expect(read("d.md")).toBe("oldApi('d');\n");
  });

  it("visits hidden files and honors .gitignore without a git repository, like the engine", async () => {
    write(".gitignore", "ignored/\n*.generated.ts\n");
    write(".hidden/h.ts", "oldApi('h');\n");
    write("ignored/i.ts", "oldApi('i');\n");
    write("src/a.ts", "oldApi('a');\n");
    write("src/x.generated.ts", "oldApi('x');\n");
    // The language's default globs are include overrides, so like the engine
    // they whitelist a gitignored file; a gitignored directory stays pruned.
    const plain = await runJssg({ name: "t", script: "transform.ts", language: "typescript" });
    expect(plain.completion.output).toEqual([
      { file: ".hidden/h.ts" },
      { file: "src/a.ts" },
      { file: "src/x.generated.ts" },
    ]);
    expect(read("ignored/i.ts")).toBe("oldApi('i');\n");
    expect(read("src/x.generated.ts")).toBe("newApi('x');\n");
    // With an exclude and no include, ignore files decide again.
    write(".hidden/h.ts", "oldApi('h');\n");
    write("src/a.ts", "oldApi('a');\n");
    write("src/x.generated.ts", "oldApi('x');\n");
    const excluded = await runJssg({
      name: "t",
      script: "transform.ts",
      language: "typescript",
      include: ["**/*.ts"],
      exclude: ["**/*.generated.ts"],
    });
    expect(excluded.completion.output).toEqual([{ file: ".hidden/h.ts" }, { file: "src/a.ts" }]);
    expect(read("ignored/i.ts")).toBe("oldApi('i');\n");
    expect(read("src/x.generated.ts")).toBe("oldApi('x');\n");
  });

  it("changes nothing when a later transform fails before commit", async () => {
    write("src/a.ts", "oldApi('a');\n");
    write("src/b.ts", "oldApi('b');\n");
    const { completion, error } = await runJssg({
      name: "t",
      script: "fail-second.ts",
      language: "typescript",
    });
    expect(completion.status).toBe("failed");
    expect(completion.error?.message).toContain("second file exploded");
    expect(completion.error?.details).toMatchObject({ phase: "transform", path: "src/b.ts" });
    expect(error).toBeInstanceOf(OperationError);
    expect(read("src/a.ts")).toBe("oldApi('a');\n");
    expect(read("src/b.ts")).toBe("oldApi('b');\n");
  });

  it("rejects conflicting rename destinations before any write", async () => {
    write("a.ts", "a\n");
    write("b.ts", "b\n");
    const { completion } = await runJssg({
      name: "t",
      script: "conflict.ts",
      language: "typescript",
    });
    expect(completion.status).toBe("failed");
    expect(completion.error?.details).toMatchObject({
      phase: "stage",
      path: "same.ts",
      origin: "b.ts",
      conflictingOrigin: "a.ts",
    });
    expect(read("a.ts")).toBe("a\n");
    expect(read("b.ts")).toBe("b\n");
    expect(existsSync(join(repo, "same.ts"))).toBe(false);
  });

  it("commits staged edits and renames, removing sources only after every file ran", async () => {
    write("src/keep.ts", "oldApi('keep');\n");
    write("src/x.old.ts", "oldApi('x');\n");
    write("src/y.old.ts", "oldApi('y');\n");
    const { completion } = await runJssg({
      name: "t",
      script: "rename.ts",
      language: "typescript",
    });
    expect(completion.status, JSON.stringify(completion)).toBe("succeeded");
    expect(completion.output).toEqual([]);
    expect(read("src/keep.ts")).toBe("newApi('keep');\n");
    expect(read("src/x.new.ts")).toBe("newApi('x');\n");
    expect(read("src/y.new.ts")).toBe("newApi('y');\n");
    expect(existsSync(join(repo, "src/x.old.ts"))).toBe(false);
    expect(existsSync(join(repo, "src/y.old.ts"))).toBe(false);
  });

  it("replays without executing after the checkout and script root move", async () => {
    write("src/a.ts", "oldApi('a');\n");
    const history = new MemoryHistoryStore();
    const first = await runJssg(
      { name: "t", script: "transform.ts", language: "typescript" },
      { target: { root: "src" } },
      { history },
    );
    expect(first.completion.status).toBe("succeeded");
    expect(JSON.stringify(await history.load())).not.toContain(scripts);

    const moved = mkdtempSync(join(tmpdir(), "codemod-moved-"));
    try {
      cpSync(scripts, join(moved, "pkg"), { recursive: true });
      const executed: OperationRequest[] = [];
      const inner = new BridgeExecutor({ bin, cwd: repo, scriptRoot: join(moved, "pkg") });
      const command = jssg({ name: "t", script: "transform.ts", language: "typescript" });
      const second = await run(
        workflow(() => command({ target: { root: "src" } })),
        {
          executor: {
            execute(request, signal) {
              executed.push(request);
              return inner.execute(request, signal);
            },
          },
          history: MemoryHistoryStore.fromJSON(history.serialize()),
        },
      );
      expect(second.replayed).toBe(true);
      expect(second.output).toEqual(first.output);
      expect(executed).toHaveLength(0);
      expect(read("src/a.ts")).toBe("newApi('a');\n");
    } finally {
      rmSync(moved, { recursive: true, force: true });
    }
  });

  it("kills the worker on abort and reports cancelled with nothing written", async () => {
    write("a.ts", "a\n");
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 500);
    const { completion, events, error } = await runJssg(
      { name: "t", script: "hang.ts", language: "typescript" },
      {},
      { signal: controller.signal },
    );
    expect(completion.status).toBe("cancelled");
    expect(completion.error?.details).toMatchObject({ committed: false });
    expect(error).toBeInstanceOf(OperationError);
    expect(read("a.ts")).toBe("a\n");
    const pid = (events.events.find((e) => e.type === "jssg.worker") as { pid?: number }).pid!;
    for (let attempt = 0; ; attempt++) {
      try {
        process.kill(pid, 0);
      } catch {
        break;
      }
      if (attempt > 50) throw new Error(`worker ${pid} still alive`);
      await new Promise((r) => setTimeout(r, 50));
    }
  });

  it("rejects a rename outside the target root on the Rust side and writes nothing", async () => {
    write("a.ts", "a\n");
    const { completion } = await runJssg({
      name: "t",
      script: "escape.ts",
      language: "typescript",
    });
    expect(completion.status).toBe("failed");
    expect(completion.error?.message).toContain("outside the target directory");
    expect(read("a.ts")).toBe("a\n");
    expect(existsSync(join(dirname(repo), "escaped.ts"))).toBe(false);
  });
});

/**
 * Cross-language test: a TypeScript workflow whose exec operation crosses the
 * Rust execution bridge binary (`butterflow-execution-bridge <request> <response>`)
 * and runs through butterflow_runners::DirectRunner.
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
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  BridgeExecutor,
  MemoryHistoryStore,
  OperationError,
  exec,
  guard,
  run,
  workflow,
  type OperationExecutor,
  type OperationRequest,
} from "../src/index.ts";

const bin =
  process.env.CODEMOD_BRIDGE_BIN ??
  resolve(import.meta.dirname, "../../../target/debug/butterflow-execution-bridge");

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
    if (!existsSync(bin)) {
      throw new Error(
        `bridge binary not found at ${bin}; run 'cargo build -p butterflow-execution-bridge' or set CODEMOD_BRIDGE_BIN`,
      );
    }
    dir = mkdtempSync(join(tmpdir(), "codemod-bridge-"));
    calls = [];
    const bridge = new BridgeExecutor({ bin: relative(process.cwd(), bin), cwd: dir });
    executor = {
      execute(request) {
        calls.push(request);
        return bridge.execute(request);
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

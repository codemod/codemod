/**
 * Cross-language test: a TypeScript workflow whose exec operation crosses the
 * Rust execution bridge binary (`butterflow-execution-bridge <request> <response>`)
 * and runs through butterflow_runners::DirectRunner.
 *
 * Run with: pnpm --filter @codemod.com/orchestration test:e2e
 * (builds only crates/execution-bridge, then runs this file).
 * Override the binary with CODEMOD_BRIDGE_BIN=/path/to/butterflow-execution-bridge.
 */
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
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

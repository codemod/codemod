import { describe, expect, it } from "vitest";
import { cancelled, createHarness, failed, unknown } from "../src/harness.ts";
import {
  DuplicateCommandIdError,
  BridgeExecutor,
  MemoryHistoryStore,
  NoActiveWorkflowError,
  OperationError,
  PROTOCOL_VERSION,
  exec,
  guard,
  jssg,
  parallel,
  plan,
  run,
  workflow,
} from "../src/index.ts";

interface Project {
  needsMigration: boolean;
  files: number;
}

const isProject = (v: unknown): v is Project =>
  typeof v === "object" && v !== null && typeof (v as Project).needsMigration === "boolean";
const Project = guard("Project", isProject);
const Summary = guard(
  "Summary",
  (v: unknown): v is { migrated: number } => typeof v === "object" && v !== null,
);

const inspect = exec({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({
  name: "migrate",
  script: "migrate.ts",
  language: "typescript",
  input: Project,
  output: Summary,
});

const migration = workflow(async () => {
  const project = await inspect();
  if (project.needsMigration) {
    const summary = await migrate({ input: project });
    return { migrated: summary.migrated, files: project.files };
  }
  return { migrated: 0, files: project.files };
});

describe("workflow execution", () => {
  it("hands typed plain data between operations and follows conditions", async () => {
    const h = createHarness({
      results: { inspect: { needsMigration: true, files: 3 }, migrate: { migrated: 3 } },
    });
    const result = await h.run(migration);

    expect(result.output).toEqual({ migrated: 3, files: 3 });
    expect(result.commands.map((c) => c.id)).toEqual(["inspect", "migrate"]);
    expect(result.commands[1]?.input).toEqual({ needsMigration: true, files: 3 });
    expect(result.commands[1]?.operation).toEqual({
      kind: "jssg",
      script: "migrate.ts",
      language: "typescript",
      input: { needsMigration: true, files: 3 },
    });
    expect(h.executed).toHaveLength(2);
  });

  it("skips the migration branch when not needed", async () => {
    const h = createHarness({ results: { inspect: { needsMigration: false, files: 1 } } });
    const result = await h.run(migration);
    expect(result.output).toEqual({ migrated: 0, files: 1 });
    expect(result.commands.map((c) => c.id)).toEqual(["inspect"]);
  });

  it("validates output schemas and rejects non-JSON exec stdout", async () => {
    const bad = createHarness({ results: { inspect: "not json" } });
    await expect(bad.run(migration)).rejects.toThrow(/stdout is not JSON/);

    const wrong = createHarness({ results: { inspect: { files: 1 } } });
    await expect(wrong.run(migration)).rejects.toThrow(/expected Project/);
  });

  it("returns raw stdout when an exec runnable has no output schema", async () => {
    const list = exec({ name: "list", command: "ls" });
    const h = createHarness({ results: { list: "a\nb\n" } });
    const result = await h.run(workflow(() => list()));
    expect(result.output).toEqual({ stdout: "a\nb\n" });
  });

  it("rejects a succeeded exec completion without string stdout", async () => {
    const executor = {
      async execute(request: { commandId: string }) {
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "succeeded" as const,
          output: null,
        };
      },
    };

    await expect(
      run(
        workflow(() => exec({ name: "step", command: "step" })()),
        { executor },
      ),
    ).rejects.toThrow("exec completion did not contain string stdout");
  });

  it("refuses to finalize when a created command was never awaited, and runs nothing", async () => {
    const h = createHarness({ fallback: () => "ok" });
    const step = exec({ name: "step", command: "step" });
    await expect(
      h.run(
        workflow(async () => {
          step();
          return "done";
        }),
      ),
    ).rejects.toThrow("workflow body returned without awaiting 1 operation(s): step");
    expect(h.executed).toHaveLength(0);
    expect(h.store.toJSON().events).toEqual([]);
  });

  it("waits for a started but un-awaited operation and does not finalize the workflow", async () => {
    const store = new MemoryHistoryStore();
    let release!: () => void;
    let started!: () => void;
    const operationStarted = new Promise<void>((resolve) => {
      started = resolve;
    });
    const executor = {
      async execute() {
        started();
        await new Promise<void>((resolve) => {
          release = resolve;
        });
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: "step",
          status: "succeeded" as const,
          output: { stdout: "ok" },
        };
      },
    };
    const result = run(
      workflow(async () => {
        // Calling then() starts the command without waiting for it.
        exec({ name: "step", command: "step" })().then(() => {});
        return "done";
      }),
      { executor, history: store },
    );

    await operationStarted;
    let settled = false;
    void result.then(
      () => {
        settled = true;
      },
      () => {
        settled = true;
      },
    );
    await Promise.resolve();
    expect(settled).toBe(false);

    release();
    await expect(result).rejects.toThrow("workflow body returned without awaiting 1 operation(s)");
    expect(store.toJSON().events.map((event) => event.type)).toEqual(["scheduled", "completed"]);
  });

  it("rejects a command issued from a stray callback after the body returned", async () => {
    const h = createHarness({ fallback: () => "ok" });
    const late = exec({ name: "late", command: "late" });
    let stray: Promise<unknown> | undefined;
    const result = await h.run(
      workflow(async () => {
        // A timer callback still sees this run's runtime, but the run is closed by then.
        stray = new Promise<void>((resolve) => setTimeout(resolve, 0)).then(() => late());
        return "done";
      }),
    );
    expect(result.output).toBe("done");
    expect(result.history.events.map((e) => e.type)).toEqual(["finalized"]);

    await expect(stray).rejects.toThrow(
      "command 'late' was issued after the workflow body returned",
    );
    expect(h.executed).toHaveLength(0);
  });

  it("rejects a command that is awaited outside any workflow", async () => {
    const step = exec({ name: "step", command: "step" });
    await expect(step()).rejects.toThrow(NoActiveWorkflowError);
    await expect(step()).rejects.toThrow(/command 'step' was awaited outside a workflow/);
    await expect(plan(step)).rejects.toThrow(/plan was awaited outside a workflow/);
    await expect(parallel(step)).rejects.toThrow(/parallel group was awaited outside a workflow/);
  });

  it("records a missing bridge binary as an unknown completion", async () => {
    const store = new MemoryHistoryStore();
    const executor = new BridgeExecutor({ bin: "missing-butterflow-execution-bridge" });

    await expect(
      run(
        workflow(() => exec({ name: "step", command: "step" })()),
        {
          executor,
          history: store,
        },
      ),
    ).rejects.toMatchObject({ status: "unknown" });
    expect(store.toJSON().events.map((event) => event.type)).toEqual(["scheduled", "completed"]);
  });

  it("serializes history, reloads it, and replays without executing", async () => {
    const h = createHarness({
      results: { inspect: { needsMigration: true, files: 2 }, migrate: { migrated: 2 } },
    });
    const first = await h.run(migration);
    expect(first.replayed).toBe(false);

    const text = h.serialize();
    const parsed = JSON.parse(text) as { protocolVersion: number; events: { type: string }[] };
    expect(parsed.protocolVersion).toBe(PROTOCOL_VERSION);
    expect(parsed.events.map((e) => e.type)).toEqual([
      "scheduled",
      "completed",
      "scheduled",
      "completed",
      "finalized",
    ]);

    const replay = createHarness({ history: text, fallback: () => failed("must not execute") });
    const second = await replay.run(migration);
    expect(second.replayed).toBe(true);
    expect(second.output).toEqual(first.output);
    expect(replay.executed).toHaveLength(0);
    expect(replay.events.events.filter((e) => e.type === "command.replayed")).toHaveLength(2);
    expect(replay.store.toJSON().events).toHaveLength(5);
  });
});

describe("command ids", () => {
  const lint = exec({ name: "lint", command: "lint" });

  it("uses explicit ids for repeated calls in a bounded loop and keeps them stable on replay", async () => {
    const loop = workflow(async () => {
      const outputs: string[] = [];
      for (let i = 0; i < 3; i++) outputs.push((await lint({ id: `lint:${i}` })).stdout);
      return outputs;
    });
    const h = createHarness({ fallback: (request) => `ran ${request.commandId}` });
    const first = await h.run(loop);
    expect(first.commands.map((c) => c.id)).toEqual(["lint:0", "lint:1", "lint:2"]);
    expect(first.output).toEqual(["ran lint:0", "ran lint:1", "ran lint:2"]);

    const second = await h.reload({ fallback: () => failed("must not execute") }).run(loop);
    expect(second.output).toEqual(first.output);
  });

  it("rejects repeated calls without an explicit id", async () => {
    const twice = workflow(async () => {
      await lint();
      await lint();
    });
    await expect(createHarness({ fallback: () => "ok" }).run(twice)).rejects.toBeInstanceOf(
      DuplicateCommandIdError,
    );
  });
});

describe("non-success outcomes", () => {
  const step = exec({ name: "step", command: "step" });
  const guarded = workflow(async () => {
    try {
      await step();
      return "ok";
    } catch (error) {
      if (error instanceof OperationError) return `${error.status}:${error.detail?.message ?? ""}`;
      throw error;
    }
  });

  it.each([
    ["failed", failed("exit 3", 3), "failed:exit 3"],
    ["cancelled", cancelled("stopped"), "cancelled:stopped"],
    ["unknown", unknown("lost"), "unknown:lost"],
  ])(
    "surfaces %s completions as OperationError and records them",
    async (status, outcome, expected) => {
      const h = createHarness({ results: { step: outcome } });
      const result = await h.run(guarded);
      expect(result.output).toBe(expected);
      expect(result.completions.get("step")?.status).toBe(status);

      const replay = await h.reload({ results: { step: "must not execute" } }).run(guarded);
      expect(replay.output).toBe(expected);
    },
  );

  it("treats a scheduled command with no recorded completion as unknown", async () => {
    const store = new MemoryHistoryStore();
    await store.append({
      type: "scheduled",
      command: {
        id: "step",
        runnable: "step",
        kind: "exec",
        operation: { kind: "exec", command: "step" },
      },
    });
    const h = createHarness({ history: store.toJSON(), results: { step: "must not execute" } });
    const result = await h.run(guarded);
    expect(result.output).toMatch(/^unknown:/);
    expect(h.executed).toHaveLength(0);
  });

  it("propagates uncaught operation failures and leaves history unfinalized", async () => {
    const h = createHarness({ results: { step: failed("boom") } });
    await expect(h.run(workflow(() => step()))).rejects.toBeInstanceOf(OperationError);
    expect(h.store.toJSON().events.map((e) => e.type)).toEqual(["scheduled", "completed"]);
  });

  it("rejects a completion for a different command", async () => {
    const store = new MemoryHistoryStore();
    const executor = {
      async execute() {
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: "other",
          status: "succeeded" as const,
          output: { stdout: "ok" },
        };
      },
    };

    await expect(
      run(
        workflow(() => step()),
        { executor, history: store },
      ),
    ).rejects.toThrow("executor returned completion for 'other' while running 'step'");
    expect(store.toJSON().events.map((event) => event.type)).toEqual(["scheduled"]);
  });
});

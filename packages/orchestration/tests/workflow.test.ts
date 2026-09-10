import { describe, expect, it } from "vitest";
import { cancelled, createHarness, failed, unknown } from "../src/harness.ts";
import {
  DuplicateCommandIdError,
  MemoryHistoryStore,
  OperationError,
  PROTOCOL_VERSION,
  exec,
  guard,
  jssg,
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
  package: "@codemod/migrate",
  input: Project,
  output: Summary,
});

const migration = workflow(async (w) => {
  const project = await w.run(inspect);
  if (project.needsMigration) {
    const summary = await w.run(migrate, { input: project });
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
      package: "@codemod/migrate",
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
    const result = await h.run(workflow((w) => w.run(list)));
    expect(result.output).toEqual({ stdout: "a\nb\n" });
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
    const loop = workflow(async (w) => {
      const outputs: string[] = [];
      for (let i = 0; i < 3; i++) outputs.push((await w.run(lint, { id: `lint:${i}` })).stdout);
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
    const twice = workflow(async (w) => {
      await w.run(lint);
      await w.run(lint);
    });
    await expect(createHarness({ fallback: () => "ok" }).run(twice)).rejects.toBeInstanceOf(
      DuplicateCommandIdError,
    );
  });
});

describe("non-success outcomes", () => {
  const step = exec({ name: "step", command: "step" });
  const guarded = workflow(async (w) => {
    try {
      await w.run(step);
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
    await expect(h.run(workflow((w) => w.run(step)))).rejects.toBeInstanceOf(OperationError);
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
        workflow((w) => w.run(step)),
        { executor, history: store },
      ),
    ).rejects.toThrow("executor returned completion for 'other' while running 'step'");
    expect(store.toJSON().events.map((event) => event.type)).toEqual(["scheduled"]);
  });
});

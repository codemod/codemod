/**
 * The run model behind the dashboard page: the static topology it renders
 * and the monitor's projected and sequenced envelopes and snapshot. Every
 * run is driven by an executor the test releases by hand, so nothing depends
 * on durations.
 */
import { describe, expect, it } from "vitest";
import {
  AdmissionScheduler,
  PROTOCOL_VERSION,
  executableIr,
  guard,
  jssg,
  parallel,
  run,
  sequence,
  shell,
  dynamic,
  type OperationCompletion,
  type OperationExecutor,
  type OperationRequest,
} from "../src/index.ts";
import { RunMonitor, projectEvent, type DashboardEnvelope } from "../src/host/dashboard/index.ts";
import { ref } from "./helpers.ts";

async function settle(turns = 8): Promise<void> {
  for (let turn = 0; turn < turns; turn++) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

/**
 * An executor whose operations stay open until the test finishes them. An
 * aborted operation completes `cancelled` on its own, as a runtime does when
 * it is killed, so a run that is aborted always settles.
 */
function controllable(): {
  executor: OperationExecutor;
  started: string[];
  open(): string[];
  finish(commandId: string, completion?: Partial<OperationCompletion>): void;
} {
  const started: string[] = [];
  const pending = new Map<string, (completion: OperationCompletion) => void>();
  return {
    executor: {
      execute(request: OperationRequest, signal?: AbortSignal) {
        started.push(request.commandId);
        return new Promise<OperationCompletion>((resolve) => {
          pending.set(request.commandId, resolve);
          signal?.addEventListener(
            "abort",
            () => {
              if (!pending.delete(request.commandId)) return;
              resolve({
                protocolVersion: PROTOCOL_VERSION,
                commandId: request.commandId,
                status: "cancelled",
                error: { message: "aborted" },
              });
            },
            { once: true },
          );
        });
      },
    },
    started,
    open: () => [...pending.keys()],
    finish(commandId, completion = {}) {
      const resolve = pending.get(commandId);
      if (resolve === undefined) throw new Error(`'${commandId}' was never started`);
      pending.delete(commandId);
      resolve({
        protocolVersion: PROTOCOL_VERSION,
        commandId,
        status: "succeeded",
        output: { stdout: commandId },
        ...completion,
      } as OperationCompletion);
    },
  };
}

/** A clock that advances one second per reading, so timestamps are deterministic. */
function ticking(): () => Date {
  let seconds = 0;
  return () => new Date(Date.UTC(2026, 0, 1, 0, 0, seconds++));
}

const step = (id: string) => shell({ name: id, command: id });
const Secret = guard(
  "Secret",
  (value: unknown): value is { token: string } => typeof value === "object" && value !== null,
);

describe("executableIr", () => {
  const migrate = jssg({ name: "migrate", language: "typescript", transform: ref("migrate") });

  it("describes every root shape as plain data", () => {
    expect(executableIr(step("only"))).toEqual({
      type: "operation",
      id: "only",
      name: "only",
      kind: "shell",
      input: "none",
    });
    expect(executableIr(shell({ name: "needs", command: "x", input: Secret }))).toMatchObject({
      type: "operation",
      id: "needs",
      input: "flow",
    });
    expect(executableIr(migrate({ id: "migrate:web", target: { root: "apps/web" } }))).toEqual({
      type: "operation",
      id: "migrate:web",
      name: "migrate",
      kind: "jssg",
      input: "none",
      target: { root: "apps/web" },
    });
    expect(executableIr(dynamic(() => 1))).toEqual({ type: "dynamic" });
    expect(
      executableIr(
        sequence(
          step("a")(),
          parallel(
            step("b")(),
            dynamic(() => 2),
          ),
          step("c")(),
        ),
      ),
    ).toEqual({
      type: "sequence",
      stages: [
        { type: "operation", id: "a", name: "a", kind: "shell", input: "none" },
        {
          type: "parallel",
          members: [
            { type: "operation", id: "b", name: "b", kind: "shell", input: "none" },
            { type: "dynamic" },
          ],
        },
        { type: "operation", id: "c", name: "c", kind: "shell", input: "none" },
      ],
    });
  });
});

describe("RunMonitor", () => {
  it("attributes runtime commands to their dynamic topology nodes", async () => {
    const workflow = sequence(
      dynamic(async () => step("first-dynamic")()),
      parallel(
        step("static")(),
        dynamic(async () => step("second-dynamic")()),
      ),
    );
    const monitor = new RunMonitor({ topology: executableIr(workflow) });
    await run(workflow, {
      executor: {
        execute: async (request) => ({
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "succeeded",
          output: { stdout: request.commandId },
        }),
      },
      events: monitor,
    });

    expect(
      monitor
        .snapshot()
        .commands.filter((command) => !command.static)
        .map(({ id, dynamicId }) => ({ id, dynamicId })),
    ).toEqual([
      { id: "first-dynamic", dynamicId: "dynamic:0.0" },
      { id: "second-dynamic", dynamicId: "dynamic:0.1.1" },
    ]);
  });

  it("projects events without env, inputs, or outputs", async () => {
    const leaky = shell({
      name: "leaky",
      command: "echo",
      input: Secret,
      env: ({ token }) => ({ TOKEN: token }),
    });
    const monitor = new RunMonitor({ topology: executableIr(leaky) });
    const envelopes: DashboardEnvelope[] = [];
    monitor.subscribe((envelope) => envelopes.push(envelope));
    await run(leaky, {
      executor: {
        execute: async (request) => ({
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "succeeded",
          output: { stdout: "hunter2-output" },
        }),
      },
      events: monitor,
      input: { token: "hunter2" },
    });

    const text = JSON.stringify([envelopes, monitor.snapshot()]);
    expect(text).not.toContain("hunter2");
    expect(text).not.toContain("TOKEN");
    expect(envelopes.map((e) => e.event.type)).toEqual([
      "command.scheduled",
      "scheduler.admitted",
      "scheduler.released",
      "command.completed",
      "run.finished",
    ]);
    expect(envelopes[0]?.event).toEqual({
      type: "command.scheduled",
      commandId: "leaky",
      runnable: "leaky",
      kind: "shell",
    });
    expect(envelopes.at(-1)?.event).toEqual({ type: "run.finished", replayed: false });
  });

  it("keeps a failure's message and phase but not its output", () => {
    expect(
      projectEvent({
        type: "command.completed",
        commandId: "x",
        completion: {
          protocolVersion: PROTOCOL_VERSION,
          commandId: "x",
          status: "failed",
          error: { message: "boom", output: "secret stdout", details: { phase: "commit" } },
        },
      }),
    ).toEqual({
      type: "command.completed",
      commandId: "x",
      status: "failed",
      error: "boom",
      phase: "commit",
    });
  });

  it("stamps a run id, a monotonic sequence, and timestamps, and folds lifecycle state", async () => {
    const now = ticking();
    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const monitor = new RunMonitor({
      topology: executableIr(parallel(step("a")(), step("b")())),
      workflow: "demo.ts",
      runId: "run-1",
      capacity: scheduler.capacity,
      now,
    });
    const envelopes: DashboardEnvelope[] = [];
    monitor.subscribe((envelope) => envelopes.push(envelope));
    const { executor, started, finish } = controllable();
    const result = run(parallel(step("a")(), step("b")()), {
      executor,
      scheduler,
      events: monitor,
    });

    await settle();
    expect(started).toEqual(["a"]);
    let snapshot = monitor.snapshot();
    expect(snapshot).toMatchObject({
      runId: "run-1",
      workflow: "demo.ts",
      status: "running",
      scheduler: { capacity: 1, used: 1, active: 1, queued: 1, paused: false },
    });
    expect(snapshot.commands.map((c) => [c.id, c.state, c.static])).toEqual([
      ["a", "running", true],
      ["b", "queued", true],
    ]);
    expect(snapshot.commands[0]).toMatchObject({
      runnable: "a",
      kind: "shell",
      concurrent: true,
      weight: 1,
      scheduledAt: "2026-01-01T00:00:01.000Z",
      startedAt: "2026-01-01T00:00:02.000Z",
    });

    finish("a");
    await settle();
    snapshot = monitor.snapshot();
    expect(snapshot.commands.map((c) => [c.id, c.state])).toEqual([
      ["a", "succeeded"],
      ["b", "running"],
    ]);
    expect(snapshot.scheduler).toMatchObject({ queued: 0, active: 1, used: 1 });
    finish("b", { status: "failed", error: { message: "boom" }, output: undefined });
    await expect(result).rejects.toThrow(/boom/u);
    monitor.fail("command 'b' failed: boom");

    snapshot = monitor.snapshot();
    expect(snapshot.status).toBe("failed");
    expect(snapshot.error).toBe("command 'b' failed: boom");
    expect(snapshot.finishedAt).toBeDefined();
    expect(monitor.finishedAt).toBe(snapshot.finishedAt);
    expect(monitor.error).toBe(snapshot.error);
    expect(monitor.startedAt).toBe("2026-01-01T00:00:00.000Z");
    expect(snapshot.commands[1]).toMatchObject({ state: "failed", error: "boom" });

    expect(envelopes.map((e) => e.seq)).toEqual(envelopes.map((_, index) => index + 1));
    expect(snapshot.seq).toBe(envelopes.length);
    expect(envelopes.every((e) => e.runId === "run-1" && e.ts.startsWith("2026-01-01T"))).toBe(
      true,
    );
    // Every envelope carries the state after its event, so a client needs no reducer.
    const admittedB = envelopes.find(
      (e) => e.event.type === "scheduler.admitted" && e.event.commandId === "b",
    );
    expect(admittedB?.command).toMatchObject({ id: "b", state: "running" });
    expect(admittedB?.scheduler).toMatchObject({ queued: 0, active: 1 });
    expect(envelopes.at(-1)).toMatchObject({ event: { type: "run.failed" }, status: "failed" });
  });

  it("separates runtime-issued commands from the static plan and shows replays", async () => {
    const body = dynamic(async () => {
      await step("first")();
      return step("second")();
    });
    const executor: OperationExecutor = {
      execute: async (request) => ({
        protocolVersion: PROTOCOL_VERSION,
        commandId: request.commandId,
        status: "succeeded",
        output: { stdout: request.commandId },
      }),
    };
    const first = new RunMonitor({ topology: executableIr(body) });
    const { history } = await run(body, { executor, events: first });
    expect(first.snapshot().topology).toEqual({ type: "dynamic" });
    expect(first.snapshot().commands.map((c) => [c.id, c.static, c.state])).toEqual([
      ["first", false, "succeeded"],
      ["second", false, "succeeded"],
    ]);

    const again = new RunMonitor({
      topology: executableIr(sequence(step("first")(), step("second")())),
    });
    const { MemoryHistoryStore } = await import("../src/index.ts");
    await run(body, { executor, events: again, history: new MemoryHistoryStore(history) });
    const snapshot = again.snapshot();
    expect(snapshot.status).toBe("completed");
    expect(snapshot.replayed).toBe(true);
    expect(snapshot.commands.map((c) => [c.id, c.static, c.state, c.status, c.runnable])).toEqual([
      ["first", true, "replayed", "succeeded", "first"],
      ["second", true, "replayed", "succeeded", "second"],
    ]);
  });

  it("tracks pause and resume as run status without touching terminal states", () => {
    const monitor = new RunMonitor({ topology: { type: "dynamic" } });
    const scheduler = new AdmissionScheduler({ capacity: 2, events: monitor });
    scheduler.pause();
    expect(monitor.status).toBe("paused");
    expect(monitor.snapshot().scheduler.paused).toBe(true);
    scheduler.resume();
    expect(monitor.status).toBe("running");
    monitor.emit({ type: "run.finished", output: null, replayed: false });
    expect(monitor.finished).toBe(true);
    scheduler.pause();
    expect(monitor.status).toBe("completed");
    monitor.fail("late");
    expect(monitor.snapshot()).toMatchObject({ status: "completed", seq: 4 });
    expect(monitor.snapshot().error).toBeUndefined();
  });

  it("marks a cancelled outcome, once", () => {
    const monitor = new RunMonitor({ topology: { type: "dynamic" } });
    monitor.fail("aborted", true);
    monitor.fail("again");
    expect(monitor.snapshot()).toMatchObject({ status: "cancelled", error: "aborted", seq: 1 });
  });

  it("removes a command from the queued count when it is cancelled before admission", () => {
    const monitor = new RunMonitor({ topology: { type: "dynamic" } });
    monitor.emit({ type: "scheduler.queued", commandId: "queued", weight: 1 });
    monitor.emit({
      type: "command.completed",
      commandId: "queued",
      completion: {
        protocolVersion: PROTOCOL_VERSION,
        commandId: "queued",
        status: "cancelled",
        error: { message: "aborted before admission" },
      },
    });
    expect(monitor.snapshot()).toMatchObject({
      scheduler: { queued: 0 },
      commands: [{ id: "queued", state: "cancelled" }],
    });
  });

  it("bounds its buffer and reports when a client fell behind", () => {
    const monitor = new RunMonitor({ topology: { type: "dynamic" }, bufferSize: 3 });
    for (let index = 0; index < 6; index++) {
      monitor.emit({ type: "scheduler.queued", commandId: `c${index}`, weight: 1 });
    }
    expect(monitor.since(6)).toEqual([]);
    expect(monitor.since(99)).toEqual([]);
    expect(monitor.since(3)?.map((e) => e.seq)).toEqual([4, 5, 6]);
    expect(monitor.since(4)?.map((e) => e.seq)).toEqual([5, 6]);
    expect(monitor.since(2)).toBeUndefined();
    expect(monitor.since(0)).toBeUndefined();
    expect(() => new RunMonitor({ topology: { type: "dynamic" }, bufferSize: 0 })).toThrow(
      /positive integer/u,
    );
  });

  it("isolates subscribers from each other and from the run", () => {
    const monitor = new RunMonitor({ topology: { type: "dynamic" } });
    const seen: number[] = [];
    monitor.subscribe(() => {
      throw new Error("bad subscriber");
    });
    const unsubscribe = monitor.subscribe((envelope) => seen.push(envelope.seq));
    monitor.emit({ type: "scheduler.queued", commandId: "c", weight: 1 });
    unsubscribe();
    monitor.emit({ type: "scheduler.queued", commandId: "d", weight: 1 });
    expect(seen).toEqual([1]);
    expect(monitor.snapshot().seq).toBe(2);
  });
});

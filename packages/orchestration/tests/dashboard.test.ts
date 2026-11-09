/**
 * The run model behind the dashboard page: the static topology it renders
 * and the monitor's projected, sequenced envelopes and snapshot. Runs that
 * need timing are driven by an executor the test releases by hand.
 */
import { describe, expect, it } from "vitest";
import {
  AdmissionScheduler,
  MemoryHistoryStore,
  PROTOCOL_VERSION,
  executableIr,
  guard,
  jssg,
  parallel,
  run,
  sequence,
  shell,
  dynamic,
  type Executable,
  type OperationCompletion,
  type OperationExecutor,
  type OperationRequest,
} from "../src/index.ts";
import { RunMonitor, projectEvent, type DashboardEnvelope } from "../src/host/dashboard/index.ts";
import { ref } from "./helpers.ts";

const settle = async () => {
  for (let turn = 0; turn < 8; turn++) await new Promise((resolve) => setImmediate(resolve));
};

const succeeded = (commandId: string, stdout = commandId): OperationCompletion => ({
  protocolVersion: PROTOCOL_VERSION,
  commandId,
  status: "succeeded",
  output: { stdout },
});

const succeeding: OperationExecutor = {
  execute: async (request) => succeeded(request.commandId),
};

/** Operations stay open until `finish`; an aborted one completes `cancelled` on its own. */
function controllable() {
  const started: string[] = [];
  const pending = new Map<string, (completion: OperationCompletion) => void>();
  const executor: OperationExecutor = {
    execute(request: OperationRequest, signal?: AbortSignal) {
      started.push(request.commandId);
      return new Promise<OperationCompletion>((resolve) => {
        pending.set(request.commandId, resolve);
        signal?.addEventListener("abort", () => {
          if (!pending.delete(request.commandId)) return;
          resolve({
            protocolVersion: PROTOCOL_VERSION,
            commandId: request.commandId,
            status: "cancelled",
            error: { message: "aborted" },
          });
        });
      });
    },
  };
  const finish = (commandId: string, completion: Partial<OperationCompletion> = {}) => {
    pending.get(commandId)!({ ...succeeded(commandId), ...completion } as OperationCompletion);
    pending.delete(commandId);
  };
  return { executor, started, finish };
}

const step = (id: string) => shell({ name: id, command: id });
const Secret = guard(
  "Secret",
  (value: unknown): value is { token: string } => typeof value === "object" && value !== null,
);
const monitorOf = (options: Partial<ConstructorParameters<typeof RunMonitor>[0]> = {}) =>
  new RunMonitor({ topology: { type: "dynamic" }, ...options });

/**
 * `a` and `b` in parallel under capacity 1, with a clock that ticks one
 * second per reading. `a` is running and `b` queued when this resolves.
 */
async function capacityOne() {
  let seconds = 0;
  const scheduler = new AdmissionScheduler({ capacity: 1 });
  const workflow = parallel(step("a")(), step("b")());
  const monitor = new RunMonitor({
    topology: executableIr(workflow),
    workflow: "demo.ts",
    runId: "run-1",
    capacity: scheduler.capacity,
    now: () => new Date(Date.UTC(2026, 0, 1, 0, 0, seconds++)),
  });
  const envelopes: DashboardEnvelope[] = [];
  monitor.subscribe((envelope) => envelopes.push(envelope));
  const { executor, started, finish } = controllable();
  const result = run(workflow, { executor, scheduler, events: monitor });
  result.catch(() => {});
  await settle();
  return { monitor, envelopes, started, finish, result };
}

describe("executableIr", () => {
  const migrate = jssg({ name: "migrate", language: "typescript", transform: ref("migrate") });
  const op = (id: string) => ({ type: "operation", id, name: id, kind: "shell", input: "none" });

  it.each<[string, Executable, unknown]>([
    ["an operation as its id, name, kind and input", step("only"), op("only")],
    [
      "an operation that needs input as taking flow input",
      shell({ name: "needs", command: "x", input: Secret }),
      { ...op("needs"), input: "flow" },
    ],
    [
      "a jssg invocation with its id and target",
      migrate({ id: "migrate:web", target: { root: "apps/web" } }),
      { ...op("migrate:web"), name: "migrate", kind: "jssg", target: { root: "apps/web" } },
    ],
    ["a dynamic node as opaque", dynamic(() => 1), { type: "dynamic" }],
    [
      "sequence and parallel in order",
      sequence(
        step("a")(),
        parallel(
          step("b")(),
          dynamic(() => 2),
        ),
      ),
      {
        type: "sequence",
        stages: [op("a"), { type: "parallel", members: [op("b"), { type: "dynamic" }] }],
      },
    ],
  ])("describes %s", (_, executable, ir) => {
    expect(executableIr(executable)).toEqual(ir);
  });
});

describe("RunMonitor projection", () => {
  it("attributes runtime commands to the dynamic node that issued them", async () => {
    const workflow = sequence(
      dynamic(async () => step("first-dynamic")()),
      parallel(
        step("static")(),
        dynamic(async () => step("second-dynamic")()),
      ),
    );
    const monitor = new RunMonitor({ topology: executableIr(workflow) });
    await run(workflow, { executor: succeeding, events: monitor });
    const dynamicCommands = monitor.snapshot().commands.filter((command) => !command.static);
    expect(dynamicCommands.map(({ id, dynamicId }) => ({ id, dynamicId }))).toEqual([
      { id: "first-dynamic", dynamicId: "dynamic:0.0" },
      { id: "second-dynamic", dynamicId: "dynamic:0.1.1" },
    ]);
  });

  it("never carries a command's env, input, or output", async () => {
    const leaky = shell({
      name: "leaky",
      command: "echo",
      input: Secret,
      env: ({ token }) => ({ TOKEN: token }),
    });
    const monitor = new RunMonitor({ topology: executableIr(leaky) });
    const envelopes: DashboardEnvelope[] = [];
    monitor.subscribe((envelope) => envelopes.push(envelope));
    const executor = { execute: async (r: OperationRequest) => succeeded(r.commandId, "hunter2") };
    await run(leaky, { executor, events: monitor, input: { token: "hunter2" } });
    expect(JSON.stringify([envelopes, monitor.snapshot()])).not.toMatch(/hunter2|TOKEN/u);
  });

  it("projects a command's lifecycle as scheduled, admitted, released, completed, finished", async () => {
    const monitor = monitorOf();
    const types: string[] = [];
    monitor.subscribe((envelope) => types.push(envelope.event.type));
    await run(step("a"), { executor: succeeding, events: monitor });
    expect(types).toEqual([
      "command.scheduled",
      "scheduler.admitted",
      "scheduler.released",
      "command.completed",
      "run.finished",
    ]);
  });

  it("projects a scheduled command as its id, runnable and kind, and a finish as replayed or not", async () => {
    const monitor = monitorOf();
    const events: unknown[] = [];
    monitor.subscribe((envelope) => events.push(envelope.event));
    await run(step("a"), { executor: succeeding, events: monitor });
    expect([events[0], events.at(-1)]).toEqual([
      { type: "command.scheduled", commandId: "a", runnable: "a", kind: "shell" },
      { type: "run.finished", replayed: false },
    ]);
  });

  it("keeps a failure's message and phase but not its output", () => {
    const completion: OperationCompletion = {
      protocolVersion: PROTOCOL_VERSION,
      commandId: "x",
      status: "failed",
      error: { message: "boom", output: "secret stdout", details: { phase: "commit" } },
    };
    expect(projectEvent({ type: "command.completed", commandId: "x", completion })).toEqual({
      type: "command.completed",
      commandId: "x",
      status: "failed",
      error: "boom",
      phase: "commit",
    });
  });
});

describe("RunMonitor snapshot of a live run", () => {
  it("shows the running command and the one waiting for capacity", async () => {
    const { monitor, started } = await capacityOne();
    expect(started).toEqual(["a"]);
    expect(monitor.snapshot()).toMatchObject({
      runId: "run-1",
      workflow: "demo.ts",
      status: "running",
      scheduler: { capacity: 1, used: 1, active: 1, queued: 1, paused: false },
      commands: [
        { id: "a", state: "running", static: true, kind: "shell", concurrent: true, weight: 1 },
        { id: "b", state: "queued", static: true },
      ],
    });
  });

  it("records when each command was scheduled and started", async () => {
    const { monitor } = await capacityOne();
    expect(monitor.startedAt).toBe("2026-01-01T00:00:00.000Z");
    expect(monitor.snapshot().commands[0]).toMatchObject({
      scheduledAt: "2026-01-01T00:00:01.000Z",
      startedAt: "2026-01-01T00:00:02.000Z",
    });
  });

  it("admits the queued command when capacity frees up", async () => {
    const { monitor, finish } = await capacityOne();
    finish("a");
    await settle();
    const snapshot = monitor.snapshot();
    expect(snapshot.commands.map((c) => [c.id, c.state])).toEqual([
      ["a", "succeeded"],
      ["b", "running"],
    ]);
    expect(snapshot.scheduler).toMatchObject({ queued: 0, active: 1, used: 1 });
  });

  it("records a failed run's error and finish time", async () => {
    const { monitor, finish, result } = await capacityOne();
    finish("a");
    await settle();
    finish("b", { status: "failed", error: { message: "boom" }, output: undefined });
    await expect(result).rejects.toThrow(/boom/u);
    monitor.fail("command 'b' failed: boom");
    const snapshot = monitor.snapshot();
    expect(snapshot).toMatchObject({ status: "failed", error: "command 'b' failed: boom" });
    expect(snapshot.commands[1]).toMatchObject({ state: "failed", error: "boom" });
    expect([monitor.finishedAt, monitor.error]).toEqual([snapshot.finishedAt, snapshot.error]);
    expect(snapshot.finishedAt).toBeDefined();
  });

  it("numbers envelopes 1, 2, 3, ... under one run id, and the snapshot has the last", async () => {
    const { monitor, envelopes, finish } = await capacityOne();
    finish("a");
    await settle();
    expect(envelopes.map((e) => [e.seq, e.runId])).toEqual(
      envelopes.map((_, index) => [index + 1, "run-1"]),
    );
    expect(monitor.snapshot().seq).toBe(envelopes.length);
  });

  it("puts the state after each event on its envelope, so a client needs no reducer", async () => {
    const { envelopes, finish } = await capacityOne();
    finish("a");
    await settle();
    const admittedB = envelopes.find(
      (e) => e.event.type === "scheduler.admitted" && e.event.commandId === "b",
    );
    expect(admittedB).toMatchObject({
      command: { id: "b", state: "running" },
      scheduler: { queued: 0, active: 1 },
    });
  });
});

describe("RunMonitor and history", () => {
  const body = dynamic(async () => {
    await step("first")();
    return step("second")();
  });

  it("marks commands a dynamic body issued as not part of the static plan", async () => {
    const monitor = new RunMonitor({ topology: executableIr(body) });
    await run(body, { executor: succeeding, events: monitor });
    expect(monitor.snapshot().commands.map((c) => [c.id, c.static, c.state])).toEqual([
      ["first", false, "succeeded"],
      ["second", false, "succeeded"],
    ]);
  });

  it("shows a replayed run's commands as replayed against the static plan", async () => {
    const { history } = await run(body, { executor: succeeding });
    const monitor = new RunMonitor({
      topology: executableIr(sequence(step("first")(), step("second")())),
    });
    await run(body, {
      executor: succeeding,
      events: monitor,
      history: new MemoryHistoryStore(history),
    });
    const snapshot = monitor.snapshot();
    expect([snapshot.status, snapshot.replayed]).toEqual(["completed", true]);
    expect(snapshot.commands.map((c) => [c.id, c.static, c.state, c.status])).toEqual([
      ["first", true, "replayed", "succeeded"],
      ["second", true, "replayed", "succeeded"],
    ]);
  });
});

describe("RunMonitor status", () => {
  it("is paused while the scheduler is paused", () => {
    const monitor = monitorOf();
    const scheduler = new AdmissionScheduler({ capacity: 2, events: monitor });
    scheduler.pause();
    expect([monitor.status, monitor.snapshot().scheduler.paused]).toEqual(["paused", true]);
    scheduler.resume();
    expect(monitor.status).toBe("running");
  });

  it("keeps a finished run's status through a later pause or failure", () => {
    const monitor = monitorOf();
    const scheduler = new AdmissionScheduler({ capacity: 2, events: monitor });
    monitor.emit({ type: "run.finished", output: null, replayed: false });
    scheduler.pause();
    monitor.fail("late");
    const { status, error } = monitor.snapshot();
    expect([status, error]).toEqual(["completed", undefined]);
  });

  it("records a cancellation once", () => {
    const monitor = monitorOf();
    monitor.fail("aborted", true);
    monitor.fail("again");
    expect(monitor.snapshot()).toMatchObject({ status: "cancelled", error: "aborted", seq: 1 });
  });

  it("drops a command from the queue when it is cancelled before admission", () => {
    const monitor = monitorOf();
    monitor.emit({ type: "scheduler.queued", commandId: "q", weight: 1 });
    const completion: OperationCompletion = {
      protocolVersion: PROTOCOL_VERSION,
      commandId: "q",
      status: "cancelled",
      error: { message: "aborted before admission" },
    };
    monitor.emit({ type: "command.completed", commandId: "q", completion });
    expect(monitor.snapshot()).toMatchObject({
      scheduler: { queued: 0 },
      commands: [{ id: "q", state: "cancelled" }],
    });
  });
});

describe("RunMonitor buffer and subscribers", () => {
  const sixEvents = () => {
    const monitor = monitorOf({ bufferSize: 3 });
    for (let index = 0; index < 6; index++) {
      monitor.emit({ type: "scheduler.queued", commandId: `c${index}`, weight: 1 });
    }
    return monitor;
  };

  it.each([
    [3, [4, 5, 6]],
    [4, [5, 6]],
    [6, []],
    [99, []],
  ])("returns what came after seq %i while it is still buffered", (seq, expected) => {
    expect(
      sixEvents()
        .since(seq)
        ?.map((e) => e.seq),
    ).toEqual(expected);
  });

  it.each([0, 2])("returns undefined after seq %i, which fell out of the buffer", (seq) => {
    expect(sixEvents().since(seq)).toBeUndefined();
  });

  it("rejects a buffer size that is not a positive integer", () => {
    expect(() => monitorOf({ bufferSize: 0 })).toThrow(/positive integer/u);
  });

  it("keeps delivering to other subscribers when one throws", () => {
    const monitor = monitorOf();
    const seen: number[] = [];
    monitor.subscribe(() => {
      throw new Error("bad subscriber");
    });
    monitor.subscribe((envelope) => seen.push(envelope.seq));
    monitor.emit({ type: "scheduler.queued", commandId: "c", weight: 1 });
    expect(seen).toEqual([1]);
  });

  it("stops delivering to a subscriber once it unsubscribes", () => {
    const monitor = monitorOf();
    const seen: number[] = [];
    const unsubscribe = monitor.subscribe((envelope) => seen.push(envelope.seq));
    monitor.emit({ type: "scheduler.queued", commandId: "c", weight: 1 });
    unsubscribe();
    monitor.emit({ type: "scheduler.queued", commandId: "d", weight: 1 });
    expect([seen, monitor.snapshot().seq]).toEqual([[1], 2]);
  });
});

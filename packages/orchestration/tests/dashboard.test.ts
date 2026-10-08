/**
 * The local run dashboard: the static topology it renders, the monitor's
 * projected and sequenced envelopes and snapshot, the session that manages
 * every run of one configuration, the HTTP surface as plain functions, and
 * the loopback host itself. Every run is driven by an executor the test
 * releases by hand, so nothing depends on durations; the executor honours the
 * abort signal so restarts settle the way a killed bridge would.
 */
import { request } from "node:http";
import { afterEach, describe, expect, it } from "vitest";
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
import {
  DashboardSession,
  RunConflictError,
  RunNotFoundError,
  RunMonitor,
  handleApi,
  matchRoute,
  openEventStream,
  parseStreamId,
  projectEvent,
  startDashboard,
  type Dashboard,
  type DashboardEnvelope,
  type DashboardSnapshot,
  type SessionNotice,
  type SessionOptions,
} from "../src/host/dashboard/index.ts";
import { canListen, ref } from "./helpers.ts";

/** Socket-backed tests need a loopback listener, which a sandbox may deny. */
const listenable = await canListen();

async function settle(turns = 8): Promise<void> {
  for (let turn = 0; turn < turns; turn++) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

/**
 * An executor whose operations stay open until the test finishes them. An
 * aborted operation completes `cancelled` on its own, as the bridge does when
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

/** A session over `parallel(a, b)` at capacity 1 with predictable run ids and time. */
function makeSession(overrides: Partial<SessionOptions> = {}) {
  const control = controllable();
  let issued = 0;
  const session = new DashboardSession({
    executable: parallel(step("a")(), step("b")()),
    workflow: "demo.ts",
    executor: () => control.executor,
    capacity: 1,
    runId: () => `run-${++issued}`,
    now: ticking(),
    ...overrides,
  });
  const notices: string[] = [];
  session.subscribe((notice) => notices.push(`${notice.type} ${notice.run.runId}`));
  /** Drive the newest run to completion. */
  const complete = async (): Promise<void> => {
    await settle();
    control.finish("a");
    await settle();
    control.finish("b");
    await session.settled();
  };
  return { session, notices, complete, ...control };
}

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

describe("DashboardSession", () => {
  it("gives every run a fresh id and executes again instead of replaying", async () => {
    const { session, started, complete, notices } = makeSession();
    expect(session.currentRunId).toBeUndefined();
    expect(session.runs()).toEqual([]);
    expect(session.snapshot()).toBeUndefined();

    const first = await session.start();
    expect(first).toMatchObject({ runId: "run-1", number: 1, status: "running" });
    expect(session.activeRunId).toBe("run-1");
    await complete();
    expect(started).toEqual(["a", "b"]);
    expect(session.run("run-1")).toMatchObject({ status: "completed", number: 1 });
    expect(session.activeRunId).toBeUndefined();
    expect(session.outcome()).toEqual({
      runId: "run-1",
      number: 1,
      output: [{ stdout: "a" }, { stdout: "b" }],
    });

    const second = await session.start();
    expect(second).toMatchObject({ runId: "run-2", number: 2, status: "running" });
    await complete();
    // Both commands ran a second time: nothing came from the first run's history.
    expect(started).toEqual(["a", "b", "a", "b"]);
    expect(session.snapshot("run-2")).toMatchObject({ runId: "run-2", replayed: false });
    expect(session.snapshot("run-2")?.commands.map((c) => c.state)).toEqual([
      "succeeded",
      "succeeded",
    ]);
    // A fresh monitor per run: the same work yields the same sequence numbers,
    // starting from zero again, and timestamps are the run's own.
    expect(session.snapshot("run-1")?.seq).toBeGreaterThan(0);
    expect(session.snapshot("run-1")?.seq).toBe(session.snapshot("run-2")?.seq);
    expect(session.snapshot("run-1")?.startedAt).not.toBe(session.snapshot("run-2")?.startedAt);

    const runs = session.runs();
    expect(runs.map((r) => [r.runId, r.number, r.status])).toEqual([
      ["run-2", 2, "completed"],
      ["run-1", 1, "completed"],
    ]);
    for (const summary of runs) {
      expect(summary.durationMs).toBe(
        Date.parse(summary.finishedAt!) - Date.parse(summary.startedAt),
      );
      expect(summary.durationMs).toBeGreaterThan(0);
    }
    expect(notices).toEqual([
      "run.created run-1",
      "run.settled run-1",
      "run.created run-2",
      "run.settled run-2",
    ]);
  });

  it("refuses a new run while one is active and serializes a double tap", async () => {
    const { session, complete } = makeSession();
    // Two simultaneous starts on an idle session create exactly one run.
    const [first, second] = await Promise.allSettled([session.start(), session.start()]);
    expect(first).toMatchObject({ status: "fulfilled", value: { runId: "run-1" } });
    expect(second.status).toBe("rejected");
    expect((second as PromiseRejectedResult).reason).toBeInstanceOf(RunConflictError);
    expect((second as PromiseRejectedResult).reason).toMatchObject({
      code: "run_active",
      httpStatus: 409,
      runId: "run-1",
      runStatus: "running",
    });
    expect(session.runs()).toHaveLength(1);

    await expect(session.start()).rejects.toThrow(/use Restart/u);
    await complete();
    await expect(session.start()).resolves.toMatchObject({ runId: "run-2" });
  });

  it("restart aborts the active run, waits for it to settle, then launches a fresh one", async () => {
    const { session, started, open, notices } = makeSession();
    await session.start();
    await settle();
    expect(started).toEqual(["a"]);
    expect(open()).toEqual(["a"]);

    const observed: string[] = [];
    session.subscribe((notice) => {
      // When the new run is announced the old one has already settled.
      if (notice.type === "run.created") {
        observed.push(`${notice.run.runId} after ${session.run("run-1")?.status}`);
      }
    });
    const restart = session.restart();
    // A second tap during the restart joins it instead of starting a third run.
    expect(session.restart()).toBe(restart);
    await expect(session.start()).rejects.toMatchObject({ code: "run_active" });

    const result = await restart;
    expect(result).toEqual({
      run: expect.objectContaining({ runId: "run-2", number: 2, status: "running" }),
      stopped: "run-1",
    });
    expect(observed).toEqual(["run-2 after cancelled"]);
    expect(notices.slice(-2)).toEqual(["run.settled run-1", "run.created run-2"]);
    const old = session.snapshot("run-1")!;
    expect(old.status).toBe("cancelled");
    expect(old.commands.map((c) => [c.id, c.state])).toEqual([
      ["a", "cancelled"],
      ["b", "cancelled"],
    ]);
    expect(old.finishedAt).toBeDefined();

    await settle();
    // Only after the old run settled did the new run take the executor.
    expect(started).toEqual(["a", "a"]);
    expect(open()).toEqual(["a"]);
    expect(session.activeRunId).toBe("run-2");
    expect(session.runs().map((r) => [r.runId, r.status])).toEqual([
      ["run-2", "running"],
      ["run-1", "cancelled"],
    ]);
  });

  it("restart of a finished run simply launches a fresh one", async () => {
    const { session, complete } = makeSession();
    await session.start();
    await complete();
    const result = await session.restart();
    expect(result).toEqual({ run: expect.objectContaining({ runId: "run-2" }) });
    expect(result).not.toHaveProperty("stopped");
    expect(session.runs().map((r) => r.status)).toEqual(["running", "completed"]);
  });

  it("keeps a bounded newest-first history and never drops the active run", async () => {
    const { session, complete } = makeSession({ runLimit: 2 });
    for (let index = 0; index < 3; index++) {
      await session.start();
      await complete();
    }
    expect(session.runs().map((r) => r.runId)).toEqual(["run-3", "run-2"]);
    expect(session.snapshot("run-1")).toBeUndefined();
    expect(session.run("run-1")).toBeUndefined();
    expect(session.outcome("run-1")).toBeUndefined();
    expect(() => session.settled("run-1")).toThrow(RunNotFoundError);

    await session.start();
    expect(session.runs().map((r) => [r.runId, r.status])).toEqual([
      ["run-4", "running"],
      ["run-3", "completed"],
    ]);
    expect(session.activeRunId).toBe("run-4");
    expect(() => new DashboardSession({ ...baseOptions(), runLimit: 0 })).toThrow(
      /positive integer/u,
    );
  });

  it("serves an archived run's final snapshot read-only while controls act on the active run", async () => {
    const { session, complete } = makeSession();
    await session.start();
    await complete();
    const archived = session.snapshot("run-1")!;
    expect(archived.status).toBe("completed");

    await session.start();
    await settle();
    expect(session.pause()).toMatchObject({
      runId: "run-2",
      status: "paused",
      scheduler: { paused: true },
    });
    expect(session.snapshot("run-2")?.status).toBe("paused");
    // The archived run is untouched by anything that happens afterwards.
    expect(session.snapshot("run-1")).toEqual(archived);
    expect(session.resume()).toMatchObject({ runId: "run-2", status: "running" });
    await complete();

    expect(() => session.pause()).toThrow(RunConflictError);
    try {
      session.resume();
    } catch (error) {
      expect(error).toMatchObject({
        code: "no_active_run",
        message: "run already completed",
        runId: "run-2",
        runStatus: "completed",
      });
    }
    expect(() => makeSession().session.pause()).toThrow(/no run has started/u);
  });

  it("close aborts the active run, waits for it, and refuses further work", async () => {
    const { session, open } = makeSession();
    await session.start();
    await settle();
    expect(open()).toEqual(["a"]);

    let closed = false;
    void session.closed.then(() => {
      closed = true;
    });
    const closing = session.close();
    expect(session.close()).toBe(closing);
    expect(session.isClosed).toBe(true);
    await closing;
    expect(closed).toBe(true);
    expect(open()).toEqual([]);
    expect(session.run("run-1")).toMatchObject({ status: "cancelled" });
    expect(session.outcome()).toMatchObject({ runId: "run-1", error: expect.any(Error) });
    await expect(session.start()).rejects.toMatchObject({ code: "session_closed" });
    await expect(session.restart()).rejects.toMatchObject({ code: "session_closed" });
    expect(session.runs()).toHaveLength(1);

    // Closing an idle session resolves at once and still refuses new runs.
    const idle = makeSession().session;
    await idle.close();
    await expect(idle.start()).rejects.toBeInstanceOf(RunConflictError);
  });

  it("records a failed run's message and hands the rejection to the host", async () => {
    const { session, finish } = makeSession();
    await session.start();
    await settle();
    finish("a", { status: "failed", error: { message: "boom" }, output: undefined });
    // The failure rejects the body; the run still waits for the member that was queued.
    await settle();
    finish("b");
    await session.settled();
    expect(session.run("run-1")).toMatchObject({
      status: "failed",
      error: expect.stringMatching(/boom/u),
    });
    expect(session.outcome()).toMatchObject({ error: expect.objectContaining({ commandId: "a" }) });
  });
});

function baseOptions(): SessionOptions {
  return { executable: step("only")(), executor: () => controllable().executor, capacity: 1 };
}

describe("dashboard api", () => {
  const json = { contentType: "application/json; charset=utf-8" };

  it("matches routes and their methods", () => {
    expect(matchRoute("/")).toEqual({ methods: { GET: "page" } });
    expect(matchRoute("/api/runs")).toEqual({ methods: { GET: "runs", POST: "start" } });
    expect(matchRoute("/api/runs/run%2D1")).toEqual({ methods: { GET: "run" }, runId: "run-1" });
    expect(matchRoute("/api/runs/")).toBeUndefined();
    expect(matchRoute("/api/runs/a/b")).toBeUndefined();
    expect(matchRoute("/api/restart")).toEqual({ methods: { POST: "restart" } });
    expect(matchRoute("/nope")).toBeUndefined();
    expect(parseStreamId("run-1:12")).toEqual({ runId: "run-1", seq: 12 });
    expect(parseStreamId("a:b:3")).toEqual({ runId: "a:b", seq: 3 });
    for (const bad of [undefined, "", "12", ":3", "run-1:", "run-1:-1", "run-1:x"]) {
      expect(parseStreamId(bad), String(bad)).toBeUndefined();
    }
  });

  it("answers with structured 404, 409, and 415 bodies", async () => {
    const { session, complete } = makeSession();
    expect(await handleApi(session, "snapshot")).toMatchObject({
      status: 404,
      body: { code: "run_not_found" },
    });
    expect(await handleApi(session, "run", { runId: "zzz" })).toEqual({
      status: 404,
      body: { error: "no run 'zzz' is retained", code: "run_not_found", runId: "zzz" },
    });
    expect(await handleApi(session, "pause", { contentType: "text/plain" })).toMatchObject({
      status: 415,
    });
    expect(await handleApi(session, "start")).toMatchObject({ status: 415 });
    expect(session.runs()).toEqual([]);
    expect(await handleApi(session, "pause", json)).toEqual({
      status: 409,
      body: { error: "no run has started", code: "no_active_run" },
    });

    expect(await handleApi(session, "start", json)).toMatchObject({
      status: 201,
      body: { run: { runId: "run-1", number: 1, status: "running" } },
    });
    expect(await handleApi(session, "start", json)).toEqual({
      status: 409,
      body: {
        error: "a run is active; use Restart to stop it and start over",
        code: "run_active",
        runId: "run-1",
        status: "running",
        restart: "/api/restart",
      },
    });
    expect(await handleApi(session, "pause", json)).toMatchObject({
      status: 200,
      body: { runId: "run-1", status: "paused", scheduler: { paused: true } },
    });
    expect(await handleApi(session, "resume", json)).toMatchObject({
      status: 200,
      body: { status: "running" },
    });
    expect(await handleApi(session, "runs")).toMatchObject({
      status: 200,
      body: { current: "run-1", active: "run-1", runs: [{ runId: "run-1" }] },
    });
    expect(await handleApi(session, "snapshot")).toMatchObject({
      status: 200,
      body: { runId: "run-1" },
    });

    expect(await handleApi(session, "restart", json)).toMatchObject({
      status: 200,
      body: { run: { runId: "run-2" }, stopped: "run-1" },
    });
    expect(await handleApi(session, "run", { runId: "run-1" })).toMatchObject({
      status: 200,
      body: { runId: "run-1", status: "cancelled" },
    });
    await complete();
    expect(await handleApi(session, "pause", json)).toEqual({
      status: 409,
      body: {
        error: "run already completed",
        code: "no_active_run",
        runId: "run-2",
        status: "completed",
      },
    });
    expect(await handleApi(session, "runs")).toMatchObject({
      body: { current: "run-2", active: null, runs: [{ runId: "run-2" }, { runId: "run-1" }] },
    });
  });
});

interface Frame {
  id?: string;
  event?: string;
  data?: unknown;
  comment?: string;
}

function parseFrame(frame: string): Frame {
  const parsed: Frame = {};
  for (const line of frame.trimEnd().split("\n")) {
    if (line.startsWith("id: ")) parsed.id = line.slice(4);
    else if (line.startsWith("event: ")) parsed.event = line.slice(7);
    else if (line.startsWith("data: ")) parsed.data = JSON.parse(line.slice(6));
    else if (line.startsWith(":")) parsed.comment = line.slice(1).trim();
  }
  return parsed;
}

describe("event stream", () => {
  function collect(session: DashboardSession, lastEventId?: string) {
    const frames: Frame[] = [];
    const detach = openEventStream(session, lastEventId, (frame) => frames.push(parseFrame(frame)));
    expect(frames.shift()).toEqual({});
    return { frames, detach, take: () => frames.splice(0) };
  }

  it("opens on the current run and catches a client up by run-scoped id", async () => {
    const { session } = makeSession();
    const early = collect(session);
    expect(early.take()).toEqual([{ comment: "waiting for the first run" }]);
    await session.start();
    await settle();
    const [notice, opening, ...rest] = early.take();
    expect(notice).toMatchObject({
      event: "run",
      data: { type: "run.created", run: { runId: "run-1" }, current: "run-1", active: "run-1" },
    });
    expect((notice!.data as { runs: unknown[] }).runs).toHaveLength(1);
    // The new run is announced before it has done anything; its work then streams live.
    expect(opening).toMatchObject({ event: "snapshot", id: "run-1:0", data: { runId: "run-1" } });
    expect(rest.length).toBeGreaterThan(0);
    expect(rest.map((f) => [f.event, f.id])).toEqual(
      rest.map((_, index) => ["event", `run-1:${index + 1}`]),
    );
    early.detach();

    const monitor = session.monitor("run-1")!;
    const base = session.snapshot()!.seq;
    expect(base).toBe(rest.length);
    monitor.emit({ type: "scheduler.queued", commandId: "x", weight: 1 });
    expect(early.take()).toEqual([]);

    // Fresh client: the snapshot includes everything so far.
    const fresh = collect(session);
    expect(fresh.take()).toMatchObject([
      { event: "snapshot", id: `run-1:${base + 1}`, data: { seq: base + 1 } },
    ]);
    monitor.emit({ type: "scheduler.queued", commandId: "y", weight: 1 });
    expect(fresh.take()).toMatchObject([
      {
        event: "event",
        id: `run-1:${base + 2}`,
        data: { seq: base + 2, event: { commandId: "y" } },
      },
    ]);
    fresh.detach();

    // Reconnect with a position in this run: only what was missed.
    const behind = collect(session, `run-1:${base}`);
    expect(behind.take().map((f) => f.id)).toEqual([`run-1:${base + 1}`, `run-1:${base + 2}`]);
    behind.detach();
    expect(collect(session, `run-1:${base + 2}`).take()).toEqual([]);

    // A position in another run, or one the buffer no longer covers, gets a snapshot.
    expect(collect(session, `other:${base}`).take()).toMatchObject([{ event: "snapshot" }]);
    const small = makeSession({ bufferSize: 2 });
    await small.session.start();
    await settle();
    expect(collect(small.session, "run-1:0").take()).toMatchObject([{ event: "snapshot" }]);
  });

  it("switches to a restarted run and never mixes the two runs' envelopes", async () => {
    const { session, complete } = makeSession();
    await session.start();
    await settle();
    const client = collect(session);
    expect(client.take()).toMatchObject([{ event: "snapshot", data: { runId: "run-1" } }]);

    await session.restart();
    const frames = client.take();
    // The old run's final envelopes, its settlement, then the new run takes over.
    const runFrames = frames.filter((f) => f.event === "run");
    expect(runFrames.map((f) => (f.data as SessionNotice).type)).toEqual([
      "run.settled",
      "run.created",
    ]);
    expect(runFrames[1]!.data).toMatchObject({
      run: { runId: "run-2" },
      stopped: "run-1",
      current: "run-2",
      active: "run-2",
      runs: [{ runId: "run-2" }, { runId: "run-1", status: "cancelled" }],
    });
    const boundary = frames.indexOf(runFrames[1]!);
    const before = frames.slice(0, boundary);
    const after = frames.slice(boundary + 1);
    const lastOfOld = before.filter((f) => f.event === "event").at(-1);
    expect(lastOfOld).toMatchObject({ data: { runId: "run-1", status: "cancelled" } });
    expect(before.every((f) => f.event === "run" || f.id?.startsWith("run-1:"))).toBe(true);
    const switched = after[0]!;
    expect(switched).toMatchObject({ event: "snapshot", data: { runId: "run-2" } });
    expect(switched.id).toMatch(/^run-2:\d+$/u);
    // Whatever the new run did while the restart resolved streams under its own id.
    expect(after.slice(1).every((f) => f.event === "event" && f.id?.startsWith("run-2:"))).toBe(
      true,
    );

    // Envelopes from the archived run no longer reach the client; the new run's do.
    session.monitor("run-1")!.emit({ type: "scheduler.queued", commandId: "ghost", weight: 1 });
    expect(client.take()).toEqual([]);
    await settle();
    session.monitor("run-2")!.emit({ type: "scheduler.queued", commandId: "z", weight: 1 });
    expect(client.take()).toMatchObject([
      { event: "event", data: { runId: "run-2", event: { commandId: "z" } } },
    ]);
    await complete();
    const settled = client.take().filter((f) => f.event === "run");
    expect(settled).toMatchObject([
      { data: { type: "run.settled", run: { runId: "run-2", status: "completed" }, active: null } },
    ]);
    client.detach();
    session.monitor("run-2")!.emit({ type: "scheduler.queued", commandId: "late", weight: 1 });
    await session.start();
    expect(client.take()).toEqual([]);
  });
});

describe.skipIf(!listenable)("dashboard server", () => {
  let dashboard: Dashboard | undefined;
  let session: DashboardSession | undefined;
  afterEach(async () => {
    await dashboard?.close();
    await session?.close();
    dashboard = undefined;
    session = undefined;
  });

  async function start(options: Partial<SessionOptions> = {}) {
    const made = makeSession(options);
    session = made.session;
    dashboard = await startDashboard({ session });
    return { ...made, url: dashboard.url };
  }

  const post = (
    url: string,
    headers: Record<string, string> = { "content-type": "application/json" },
  ) => fetch(url, { method: "POST", headers });

  const getWithHost = (url: string, host: string): Promise<number> =>
    new Promise((resolve, reject) => {
      const parsed = new URL(url);
      const req = request(
        {
          hostname: parsed.hostname,
          port: parsed.port,
          path: parsed.pathname,
          method: "GET",
          headers: { host },
        },
        (res) => {
          res.resume();
          res.once("end", () => resolve(res.statusCode ?? 0));
        },
      );
      req.once("error", reject);
      req.end();
    });

  it("binds to loopback on a free port and serves the page and the snapshot", async () => {
    const { session, url } = await start();
    expect(url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/$/u);
    const page = await fetch(url);
    expect(page.headers.get("content-type")).toBe("text/html; charset=utf-8");
    const html = await page.text();
    for (const words of [
      "Steps",
      "Pause",
      "Details",
      "Sequence",
      "Parallel",
      "Restart",
      "Run again",
      "Recent runs",
      "Back to current",
    ]) {
      expect(html).toContain(words);
    }
    expect(html).toContain("Commands will appear here");
    expect(html).toContain("flow-parallel-branches");
    expect(html).toContain("flow-dynamic-commands");
    expect(html).not.toContain("Added while running");
    const markup = html.replace(/<(script|style)>[\s\S]*?<\/\1>/gu, "");
    expect(markup).not.toMatch(
      /admission|topology|scheduler|capacity|runtime-issued|event stream/iu,
    );
    expect(html).not.toMatch(/<script[^>]+src=/u);
    expect(html).not.toMatch(/<link[^>]+href=/u);

    expect((await fetch(`${url}api/snapshot`)).status).toBe(404);
    await session.start();
    const snapshot = (await (await fetch(`${url}api/snapshot`)).json()) as DashboardSnapshot;
    expect(snapshot).toMatchObject({ runId: "run-1", status: "running" });
    expect(snapshot.topology).toMatchObject({ type: "parallel" });
  });

  it("pauses and resumes admission over HTTP and refuses once the run is over", async () => {
    const { session, complete, url } = await start();
    await session.start();
    await settle();
    const scheduler = () => session.snapshot()!.scheduler;

    const noType = await post(`${url}api/pause`, {});
    expect(noType.status).toBe(415);
    expect(scheduler().paused).toBe(false);

    const paused = await post(`${url}api/pause`);
    expect(paused.status).toBe(200);
    expect(await paused.json()).toMatchObject({
      runId: "run-1",
      status: "paused",
      scheduler: { paused: true },
    });
    expect(scheduler().paused).toBe(true);

    const resumed = await post(`${url}api/resume`);
    expect(await resumed.json()).toMatchObject({ status: "running", scheduler: { paused: false } });
    expect(scheduler().paused).toBe(false);

    await complete();
    const late = await post(`${url}api/pause`);
    expect(late.status).toBe(409);
    expect(await late.json()).toEqual({
      error: "run already completed",
      code: "no_active_run",
      runId: "run-1",
      status: "completed",
    });
  });

  it("lists runs, serves archived snapshots, and starts or restarts runs", async () => {
    const { complete, url } = await start();
    const created = await post(`${url}api/runs`);
    expect(created.status).toBe(201);
    expect(await created.json()).toMatchObject({ run: { runId: "run-1", number: 1 } });
    const conflict = await post(`${url}api/runs`);
    expect(conflict.status).toBe(409);
    expect(await conflict.json()).toMatchObject({ code: "run_active", restart: "/api/restart" });

    const restarted = await post(`${url}api/restart`);
    expect(restarted.status).toBe(200);
    expect(await restarted.json()).toMatchObject({ run: { runId: "run-2" }, stopped: "run-1" });
    await complete();

    const list = (await (await fetch(`${url}api/runs`)).json()) as {
      current: string;
      active: string | null;
      runs: { runId: string; status: string }[];
    };
    expect(list.current).toBe("run-2");
    expect(list.active).toBeNull();
    expect(list.runs.map((r) => [r.runId, r.status])).toEqual([
      ["run-2", "completed"],
      ["run-1", "cancelled"],
    ]);
    const archived = await fetch(`${url}api/runs/run-1`);
    expect(archived.status).toBe(200);
    expect(await archived.json()).toMatchObject({ runId: "run-1", status: "cancelled" });
    const missing = await fetch(`${url}api/runs/nope`);
    expect(missing.status).toBe(404);
    expect(await missing.json()).toMatchObject({ code: "run_not_found", runId: "nope" });

    const again = await post(`${url}api/runs`);
    expect(again.status).toBe(201);
    expect(await again.json()).toMatchObject({ run: { runId: "run-3" } });
  });

  it("rejects unknown routes, wrong methods, and foreign host names", async () => {
    const { url } = await start();
    expect((await fetch(`${url}nope`)).status).toBe(404);
    const wrongMethod = await fetch(`${url}api/pause`);
    expect(wrongMethod.status).toBe(405);
    expect(wrongMethod.headers.get("allow")).toBe("POST");
    expect((await post(`${url}api/snapshot`)).status).toBe(405);
    const runs = await fetch(`${url}api/runs`, { method: "PUT" });
    expect(runs.status).toBe(405);
    expect(runs.headers.get("allow")).toBe("GET, POST");
    expect(await getWithHost(`${url}api/runs`, "evil.example:80")).toBe(403);
    expect(await getWithHost(`${url}api/runs`, `localhost:${dashboard!.port}`)).toBe(200);
  });

  it("streams a snapshot then live envelopes, and catches a reconnecting client up", async () => {
    const { session, url } = await start();
    await session.start();
    await settle();
    const monitor = session.monitor("run-1")!;
    const base = session.snapshot()!.seq;

    const first = await openStream(`${url}api/events`);
    const opening = await first.next();
    expect(opening.event).toBe("snapshot");
    expect(opening.id).toBe(`run-1:${base}`);
    expect(JSON.parse(opening.data)).toMatchObject({ runId: "run-1", seq: base });

    monitor.emit({ type: "scheduler.queued", commandId: "b2", weight: 1 });
    const live = await first.next();
    expect(live.event).toBe("event");
    expect(live.id).toBe(`run-1:${base + 1}`);
    expect(JSON.parse(live.data)).toMatchObject({ seq: base + 1, event: { commandId: "b2" } });
    first.close();

    // Reconnecting with the last id seen replays only what was missed.
    monitor.emit({ type: "scheduler.queued", commandId: "c", weight: 1 });
    monitor.emit({ type: "scheduler.queued", commandId: "d", weight: 1 });
    const second = await openStream(`${url}api/events`, { "last-event-id": `run-1:${base + 1}` });
    const caught = [await second.next(), await second.next()];
    expect(caught.map((m) => [m.event, m.id])).toEqual([
      ["event", `run-1:${base + 2}`],
      ["event", `run-1:${base + 3}`],
    ]);
    second.close();

    // A client that is fully caught up gets nothing until the next event.
    const third = await openStream(`${url}api/events`, { "last-event-id": `run-1:${base + 3}` });
    monitor.emit({ type: "scheduler.queued", commandId: "e", weight: 1 });
    expect((await third.next()).id).toBe(`run-1:${base + 4}`);
    third.close();
  });

  it("follows a restart without a reload and resets a stale Last-Event-ID", async () => {
    const { session, url } = await start();
    await session.start();
    await settle();
    const stream = await openStream(`${url}api/events`);
    expect((await stream.next()).event).toBe("snapshot");

    const response = await post(`${url}api/restart`);
    expect(response.status).toBe(200);
    const messages: SseMessage[] = [];
    let message = await stream.next();
    while (message.event !== "snapshot") {
      messages.push(message);
      message = await stream.next();
    }
    expect(messages.filter((m) => m.event === "run").map((m) => JSON.parse(m.data).type)).toEqual([
      "run.settled",
      "run.created",
    ]);
    expect(message.id).toMatch(/^run-2:\d+$/u);
    expect(JSON.parse(message.data)).toMatchObject({ runId: "run-2", status: "running" });
    stream.close();

    const stale = await openStream(`${url}api/events`, { "last-event-id": "run-1:3" });
    const reset = await stale.next();
    expect(reset.event).toBe("snapshot");
    expect(JSON.parse(reset.data)).toMatchObject({ runId: "run-2" });
    stale.close();
  });

  it("sends a fresh snapshot when a reconnecting client is beyond the buffer", async () => {
    const { session, url } = await start({ bufferSize: 2 });
    await session.start();
    await settle();
    const stream = await openStream(`${url}api/events`, { "last-event-id": "run-1:1" });
    const opening = await stream.next();
    expect(opening.event).toBe("snapshot");
    expect(JSON.parse(opening.data)).toMatchObject({ runId: "run-1" });
    stream.close();
  });

  it("ends open streams when it closes", async () => {
    const { session, url } = await start();
    await session.start();
    const stream = await openStream(`${url}api/events`);
    await stream.next();
    await dashboard!.close();
    dashboard = undefined;
    await expect(stream.next()).rejects.toThrow(/stream ended/u);
  });
});

interface SseMessage {
  id?: string;
  event?: string;
  data: string;
}

/** A minimal SSE reader over `fetch`: `next()` resolves with the next message. */
async function openStream(
  url: string,
  headers: Record<string, string> = {},
): Promise<{ next(): Promise<SseMessage>; close(): void }> {
  const controller = new AbortController();
  const response = await fetch(url, { headers, signal: controller.signal });
  expect(response.headers.get("content-type")).toBe("text/event-stream");
  const reader = response.body!.getReader();
  const decoder = new TextDecoder();
  let buffered = "";
  const queue: SseMessage[] = [];
  let ended = false;

  const pump = async (): Promise<void> => {
    while (queue.length === 0 && !ended) {
      const { value, done } = await reader.read();
      if (done) {
        ended = true;
        break;
      }
      buffered += decoder.decode(value, { stream: true });
      let boundary = buffered.indexOf("\n\n");
      while (boundary !== -1) {
        const block = buffered.slice(0, boundary);
        buffered = buffered.slice(boundary + 2);
        const message: SseMessage = { data: "" };
        for (const line of block.split("\n")) {
          if (line.startsWith("id: ")) message.id = line.slice(4);
          else if (line.startsWith("event: ")) message.event = line.slice(7);
          else if (line.startsWith("data: ")) message.data += line.slice(6);
        }
        // `retry:` and comment blocks carry no data and are not messages.
        if (message.data !== "") queue.push(message);
        boundary = buffered.indexOf("\n\n");
      }
    }
  };

  return {
    async next() {
      await pump();
      const message = queue.shift();
      if (message === undefined) throw new Error("stream ended");
      return message;
    },
    close: () => controller.abort(),
  };
}

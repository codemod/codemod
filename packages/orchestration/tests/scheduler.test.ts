/**
 * Bounded admission: `parallel()` declares eligibility, the run's scheduler
 * decides how much of it overlaps. Every test drives admission with an
 * executor the test releases by hand, so nothing here depends on durations.
 */
import { describe, expect, it } from "vitest";
import { createHarness, failed } from "../src/host/harness.ts";
import {
  AdmissionScheduler,
  CAPACITY_ENV,
  DEFAULT_WEIGHTS,
  MemoryHistoryStore,
  OperationError,
  PROTOCOL_VERSION,
  defaultCapacity,
  shell,
  jssg,
  parallel,
  run,
  sequence,
  weightOf,
  dynamic,
  type Operation,
  type OperationCompletion,
  type OperationExecutor,
  type OperationRequest,
  type SchedulerHost,
  type RunEvent,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

/** Drain the microtask queue and a few macrotask turns; no durations involved. */
async function settle(turns = 8): Promise<void> {
  for (let turn = 0; turn < turns; turn++) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

/**
 * An executor that records what actually reached it and never finishes on its
 * own: the test decides when each command completes.
 */
function controllable(): {
  executor: OperationExecutor;
  started: string[];
  finish(commandId: string, completion?: Partial<OperationCompletion>): void;
  throwOnLaunch: Set<string>;
} {
  const started: string[] = [];
  const pending = new Map<string, (completion: OperationCompletion) => void>();
  const throwOnLaunch = new Set<string>();
  const executor: OperationExecutor = {
    execute(request: OperationRequest) {
      started.push(request.commandId);
      if (throwOnLaunch.has(request.commandId)) {
        return Promise.reject(new Error(`could not launch ${request.commandId}`));
      }
      return new Promise<OperationCompletion>((resolve) => pending.set(request.commandId, resolve));
    },
  };
  return {
    executor,
    started,
    throwOnLaunch,
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

const step = (id: string) => shell({ name: id, command: id });
const steps = (count: number) => Array.from({ length: count }, (_, index) => step(`s${index}`));

const fileJssg = jssg({ name: "file-pass", language: "typescript", transform: ref("file-pass") });
const workspaceJssg = jssg({
  name: "workspace-pass",
  language: "typescript",
  transform: ref("workspace-pass"),
  semanticAnalysis: "workspace",
});

describe("operation weights", () => {
  it.each<[string, Operation, number]>([
    ["shell", { kind: "shell", command: "x" }, 1],
    ["agent", { kind: "agent", prompt: "x", backend: { kind: "builtin", tools: [] } }, 1],
    [
      "assessment",
      { kind: "assessment", state: "s", questions: { ok: { type: "noul", instructions: "ok?" } } },
      1,
    ],
    ["jssg without semantics", fileJssg.toOperation(undefined), DEFAULT_WEIGHTS.jssg],
    [
      "jssg with file semantics",
      { ...fileJssg.toOperation(undefined), semanticAnalysis: "file" },
      DEFAULT_WEIGHTS.jssg,
    ],
    ["jssg with workspace semantics", workspaceJssg.toOperation(undefined), 4],
    [
      "jssg with a workspace root",
      { ...fileJssg.toOperation(undefined), semanticAnalysis: { mode: "workspace", root: "src" } },
      4,
    ],
  ])("charges %s %i units", (_label, operation, expected) => {
    expect(weightOf(operation)).toBe(expected);
  });
});

describe("default capacity", () => {
  const host = (cpus: number, gibibytes: number): SchedulerHost => ({
    availableParallelism: () => cpus,
    totalmem: () => gibibytes * 1024 * 1024 * 1024,
  });

  it.each<[string, SchedulerHost, number]>([
    // Plenty of memory: the CPU count decides.
    ["a large host", host(16, 64), 16],
    ["a laptop", host(10, 32), 10],
    // A heavy operation is clamped to this capacity and can still run alone.
    ["a tiny host", host(1, 8), 1],
    ["a three-core host", host(3, 8), 3],
    // 2 GiB budget / 512 MiB per workspace pass = 4 passes = 16 units, so memory does not bind.
    ["a small-memory host", host(8, 4), 8],
    // 0.5 GiB budget = 1 pass = 4 units, below the CPU count.
    ["a memory-starved host", host(8, 1), 4],
  ])("on %s admits %i units", (_label, hostFacts, expected) => {
    expect(defaultCapacity(DEFAULT_WEIGHTS, hostFacts, {})).toBe(expected);
  });

  it("honors the operator override and rejects nonsense", () => {
    const hostFacts = host(16, 64);
    expect(defaultCapacity(DEFAULT_WEIGHTS, hostFacts, { [CAPACITY_ENV]: "3" })).toBe(3);
    expect(defaultCapacity(DEFAULT_WEIGHTS, hostFacts, { [CAPACITY_ENV]: "  " })).toBe(16);
    for (const bad of ["0", "-2", "1.5", "many"]) {
      expect(() => defaultCapacity(DEFAULT_WEIGHTS, hostFacts, { [CAPACITY_ENV]: bad })).toThrow(
        /must be a positive integer/u,
      );
    }
  });

  it("rejects a capacity that cannot admit anything", () => {
    expect(() => new AdmissionScheduler({ capacity: 0 })).toThrow(/positive integer/u);
  });
});

describe("bounded admission", () => {
  it("never exceeds capacity however many members a group declares", async () => {
    const members = steps(40).map((runnable) => runnable());
    const scheduler = new AdmissionScheduler({ capacity: 5 });
    const { executor, started, finish } = controllable();
    const result = run(
      dynamic(() => parallel(members)),
      { executor, scheduler },
    );

    await settle();
    expect(started).toEqual(["s0", "s1", "s2", "s3", "s4"]);
    expect(scheduler.stats()).toMatchObject({ capacity: 5, used: 5, active: 5, queued: 35 });

    // Each release admits exactly one more, in declaration order.
    for (const id of started.slice()) finish(id);
    await settle();
    expect(started.slice(5)).toEqual(["s5", "s6", "s7", "s8", "s9"]);

    for (let index = 5; index < 40; index++) {
      finish(`s${index}`);
      await settle(2);
    }
    const { output } = await result;
    expect((output as { stdout: string }[]).map((o) => o.stdout)).toEqual(
      members.map((member) => member.runnable.name),
    );
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0, peakActive: 5 });
  });

  it("charges a workspace JSSG pass more than ordinary work", async () => {
    // Capacity 5 fits one workspace pass (4) plus one shell (1), but never two
    // workspace passes, where 40 ordinary execs would have fit five at a time.
    const scheduler = new AdmissionScheduler({ capacity: 5 });
    const { executor, started, finish } = controllable();
    const group = parallel(
      workspaceJssg({ id: "w0" }),
      workspaceJssg({ id: "w1" }),
      step("e0")(),
      fileJssg({ id: "f0" }),
    );
    const result = run(
      dynamic(() => group),
      { executor, scheduler },
    );

    await settle();
    // w1 needs 4 of the 5 units; only 1 is free, and strict FIFO keeps e0 and
    // f0 behind it rather than letting them overtake a heavy command.
    expect(started).toEqual(["w0"]);
    expect(scheduler.stats()).toMatchObject({ used: 4, active: 1, queued: 3 });

    finish("w0");
    await settle();
    // The second workspace pass takes 4 of 5 units; only the shell fits beside it.
    expect(started).toEqual(["w0", "w1", "e0"]);
    expect(scheduler.stats()).toMatchObject({ used: 5, active: 2, queued: 1 });

    finish("w1");
    await settle();
    expect(started).toEqual(["w0", "w1", "e0", "f0"]);
    expect(scheduler.stats()).toMatchObject({ used: 3, active: 2, queued: 0 });

    finish("e0");
    finish("f0", { output: [] });
    await expect(result).resolves.toBeDefined();
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, peakUsed: 5, peakActive: 2 });
  });

  it("returns outputs in declaration order when completion order is reversed", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 3 });
    const { executor, started, finish } = controllable();
    const result = run(
      dynamic(() => parallel(steps(3).map((runnable) => runnable()))),
      { executor, scheduler },
    );

    await settle();
    expect(started).toEqual(["s0", "s1", "s2"]);
    for (const id of ["s2", "s1", "s0"]) finish(id, { output: { stdout: `done ${id}` } });

    const { output, history } = await result;
    expect((output as { stdout: string }[]).map((o) => o.stdout)).toEqual([
      "done s0",
      "done s1",
      "done s2",
    ]);
    // Commands are still recorded in declaration order, so replay is unaffected.
    const scheduled = history.events.filter((event) => event.type === "scheduled");
    expect(scheduled.map((event) => (event.type === "scheduled" ? event.command.id : ""))).toEqual([
      "s0",
      "s1",
      "s2",
    ]);
  });

  it("does not reach the executor before a queued command is admitted", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const events: RunEvent[] = [];
    const { executor, started, finish } = controllable();
    const result = run(
      dynamic(() => parallel(steps(3).map((runnable) => runnable()))),
      {
        executor,
        scheduler,
        events: { emit: (event) => events.push(event) },
      },
    );

    await settle();
    expect(started).toEqual(["s0"]);
    expect(events.filter((e) => e.type === "scheduler.queued").map((e) => e.commandId)).toEqual([
      "s1",
      "s2",
    ]);
    // The queued commands are recorded in history but have touched no executor.
    expect(events.filter((e) => e.type === "command.scheduled")).toHaveLength(3);

    finish("s0");
    await settle();
    expect(started).toEqual(["s0", "s1"]);
    finish("s1");
    await settle();
    finish("s2");
    await expect(result).resolves.toBeDefined();
    expect(events.filter((e) => e.type === "scheduler.admitted")).toHaveLength(3);
  });
});

describe("cancellation", () => {
  it("refuses a queued command without starting it and keeps killing admitted ones", async () => {
    const controller = new AbortController();
    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const { executor, started, finish } = controllable();
    const history = new MemoryHistoryStore();
    const result = run(
      dynamic(() => parallel(steps(2).map((runnable) => runnable()))),
      {
        executor,
        scheduler,
        history,
        signal: controller.signal,
      },
    );
    const caught = result.catch((error: unknown) => error);

    await settle();
    expect(started).toEqual(["s0"]);
    controller.abort();
    await settle();
    // s1 was refused while queued: no executor call, so no bridge process.
    expect(started).toEqual(["s0"]);
    expect(scheduler.stats()).toMatchObject({ queued: 0, active: 1 });

    // The admitted command still runs to whatever its executor reports.
    finish("s0", { status: "cancelled", error: { message: "killed" }, output: undefined });
    const error = await caught;
    expect(error).toBeInstanceOf(OperationError);

    const recorded = (await history.load()).events;
    expect(recorded.map((event) => event.type)).toEqual([
      "scheduled",
      "scheduled",
      "completed",
      "completed",
    ]);
    const s1 = recorded.find((event) => event.type === "completed" && event.commandId === "s1");
    expect(s1).toMatchObject({
      completion: {
        status: "cancelled",
        error: { message: "cancelled while queued for execution capacity" },
      },
    });
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0 });
  });

  it("refuses admission outright when the signal already aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    const scheduler = new AdmissionScheduler({ capacity: 4 });
    const { executor, started } = controllable();
    await expect(
      run(
        dynamic(() => step("only")()),
        { executor, scheduler, signal: controller.signal },
      ),
    ).rejects.toBeInstanceOf(OperationError);
    expect(started).toEqual([]);
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, peakActive: 0 });
  });
});

describe("pause and resume", () => {
  it("holds every admission while paused and admits in declaration order on resume", async () => {
    const events: RunEvent[] = [];
    const sink = { emit: (event: RunEvent) => events.push(event) };
    const scheduler = new AdmissionScheduler({ capacity: 3, events: sink });
    const { executor, started, finish } = controllable();
    scheduler.pause();
    const result = run(
      dynamic(() => parallel(steps(3).map((runnable) => runnable()))),
      { executor, scheduler, events: sink },
    );

    await settle();
    // Capacity is free, but nothing is admitted: the commands are queued.
    expect(started).toEqual([]);
    expect(scheduler.stats()).toMatchObject({ paused: true, queued: 3, active: 0, used: 0 });
    expect(events.filter((e) => e.type === "scheduler.queued")).toHaveLength(3);
    expect(events.find((e) => e.type === "scheduler.paused")).toEqual({
      type: "scheduler.paused",
      queued: 0,
      active: 0,
    });

    scheduler.resume();
    await settle();
    expect(started).toEqual(["s0", "s1", "s2"]);
    expect(events.find((e) => e.type === "scheduler.resumed")).toEqual({
      type: "scheduler.resumed",
      queued: 3,
      active: 0,
    });
    expect(scheduler.stats()).toMatchObject({ paused: false, queued: 0, active: 3 });
    for (const id of ["s0", "s1", "s2"]) finish(id);
    await expect(result).resolves.toBeDefined();
  });

  it("lets admitted operations finish without admitting the next one", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const { executor, started, finish } = controllable();
    const result = run(
      dynamic(() => parallel(steps(2).map((runnable) => runnable()))),
      { executor, scheduler },
    );

    await settle();
    expect(started).toEqual(["s0"]);
    scheduler.pause();
    finish("s0");
    await settle();
    // s0 completed and returned its permit, but s1 stays queued.
    expect(started).toEqual(["s0"]);
    expect(scheduler.stats()).toMatchObject({ paused: true, used: 0, active: 0, queued: 1 });

    scheduler.resume();
    await settle();
    expect(started).toEqual(["s0", "s1"]);
    finish("s1");
    await expect(result).resolves.toBeDefined();
  });

  it("is idempotent and still refuses a queued command that is aborted while paused", async () => {
    const events: RunEvent[] = [];
    const sink = { emit: (event: RunEvent) => events.push(event) };
    const controller = new AbortController();
    const scheduler = new AdmissionScheduler({ capacity: 1, events: sink });
    const { executor, started, finish } = controllable();
    const result = run(
      dynamic(() => parallel(steps(2).map((runnable) => runnable()))),
      { executor, scheduler, signal: controller.signal, events: sink },
    );
    const caught = result.catch((error: unknown) => error);

    await settle();
    scheduler.pause();
    scheduler.pause();
    scheduler.resume();
    scheduler.resume();
    scheduler.pause();
    expect(events.filter((e) => e.type === "scheduler.paused")).toHaveLength(2);
    expect(events.filter((e) => e.type === "scheduler.resumed")).toHaveLength(1);

    controller.abort();
    await settle();
    expect(started).toEqual(["s0"]);
    expect(scheduler.stats()).toMatchObject({ paused: true, queued: 0, active: 1 });
    finish("s0", { status: "cancelled", error: { message: "killed" }, output: undefined });
    expect(await caught).toBeInstanceOf(OperationError);
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0 });
  });
});

describe("permits", () => {
  it.each<[string, Partial<{ fail: boolean; abort: boolean; launchError: boolean }>]>([
    ["success", {}],
    ["an operation failure", { fail: true }],
    ["cancellation", { abort: true }],
    ["an executor launch failure", { launchError: true }],
  ])("are released after %s", async (_label, mode) => {
    const scheduler = new AdmissionScheduler({ capacity: 2 });
    const { executor, throwOnLaunch, finish } = controllable();
    const controller = new AbortController();
    if (mode.launchError) throwOnLaunch.add("only");
    const result = run(
      dynamic(() => step("only")()),
      {
        executor,
        scheduler,
        signal: controller.signal,
      },
    );
    const settled = result.catch((error: unknown) => error);

    await settle();
    if (mode.abort) {
      controller.abort();
      finish("only", { status: "cancelled", error: { message: "killed" }, output: undefined });
    } else if (mode.fail) {
      finish("only", { status: "failed", error: { message: "boom" }, output: undefined });
    } else if (!mode.launchError) {
      finish("only");
    }
    await settled;

    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0, peakActive: 1 });
  });
});

describe("replay", () => {
  it("consumes no capacity and never reaches the executor", async () => {
    const members = steps(4).map((runnable) => runnable());
    const first = createHarness({ fallback: (request) => `ran ${request.commandId}` });
    const body = dynamic(() => parallel(members));
    const recorded = await first.run(body);

    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const replay = createHarness({
      history: first.serialize(),
      fallback: () => failed("must not execute"),
      scheduler,
    });
    const second = await replay.run(body);

    expect(second.replayed).toBe(true);
    expect(second.output).toEqual(recorded.output);
    expect(replay.executed).toHaveLength(0);
    // Capacity 1 would have serialized four members; none of them needed it.
    expect(scheduler.stats()).toMatchObject({ peakActive: 0, peakUsed: 0, used: 0 });
  });
});

describe("group shapes", () => {
  it("bounds dynamically built and repeated groups through one run scheduler", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 2 });
    const { executor, started, finish } = controllable();
    const early = steps(3);
    const late = steps(3).map((runnable, index) => runnable({ id: `late${index}` }));
    // Two groups awaited at once: the second is built from an array inside the body.
    const body = dynamic(async () => {
      const [a, b] = await Promise.all([
        parallel(early.map((runnable, index) => runnable({ id: `early${index}` }))),
        parallel(late),
      ]);
      return [a, b];
    });
    const result = run(body, { executor, scheduler });

    await settle();
    expect(started).toHaveLength(2);
    for (let admitted = 0; admitted < 6; admitted++) {
      finish(started[admitted]!);
      await settle(2);
    }
    await expect(result).resolves.toBeDefined();
    expect(started).toHaveLength(6);
    expect(scheduler.stats()).toMatchObject({ peakActive: 2, used: 0, active: 0, queued: 0 });
  });

  it("runs a sequence's parallel stages under the same bound and keeps order", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 2 });
    const harness = createHarness({
      fallback: (request) => `ran ${request.commandId}`,
      scheduler,
    });
    const fixed = sequence(parallel(steps(3).map((runnable) => runnable())), step("last")());
    const result = await harness.run(fixed);
    expect(result.commands.map((command) => command.id)).toEqual(["s0", "s1", "s2", "last"]);
    expect(scheduler.stats()).toMatchObject({ peakActive: 2, used: 0, queued: 0 });
  });

  it("does not change a sequential workflow at capacity 1", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const first = step("first");
    const second = step("second");
    const harness = createHarness({
      fallback: (request) => `ran ${request.commandId}`,
      scheduler,
    });
    const result = await harness.run(
      dynamic(async () => [(await first()).stdout, (await second()).stdout]),
    );
    expect(result.output).toEqual(["ran first", "ran second"]);
    expect(scheduler.stats()).toMatchObject({ peakActive: 1, used: 0 });
  });

  it("keeps two runs independent of each other", async () => {
    const a = new AdmissionScheduler({ capacity: 1 });
    const b = new AdmissionScheduler({ capacity: 3 });
    const left = controllable();
    const right = controllable();
    const body = dynamic(() => parallel(steps(3).map((runnable) => runnable())));
    const first = run(body, { executor: left.executor, scheduler: a });
    const second = run(body, { executor: right.executor, scheduler: b });

    await settle();
    expect(a.stats()).toMatchObject({ active: 1, queued: 2 });
    expect(b.stats()).toMatchObject({ active: 3, queued: 0 });

    for (const control of [left, right]) {
      for (let index = 0; index < 3; index++) {
        control.finish(control.started[index]!);
        await settle(2);
      }
    }
    await expect(Promise.all([first, second])).resolves.toBeDefined();
  });
});

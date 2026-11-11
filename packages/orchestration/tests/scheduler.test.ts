/**
 * Bounded admission: `parallel()` declares eligibility, the run's scheduler
 * decides how much of it overlaps. Every test drives admission with an
 * executor the test releases by hand, so nothing here depends on durations.
 */
import { describe, expect, it } from "vitest";
import { createHarness, failed } from "./support/harness.ts";
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
  type Executable,
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

/** Records what reached it; each command stays open until the test calls `finish`. */
function controllable(unlaunchable: string[] = []) {
  const started: string[] = [];
  const pending = new Map<string, (completion: OperationCompletion) => void>();
  const executor: OperationExecutor = {
    execute(request: OperationRequest) {
      started.push(request.commandId);
      if (unlaunchable.includes(request.commandId)) {
        return Promise.reject(new Error(`could not launch ${request.commandId}`));
      }
      return new Promise<OperationCompletion>((resolve) => pending.set(request.commandId, resolve));
    },
  };
  const finish = (commandId: string, completion: Partial<OperationCompletion> = {}) => {
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
  };
  return { executor, started, finish };
}
type Control = ReturnType<typeof controllable>;

/** Finishes `count` commands one at a time, in the order they started. */
async function drain(
  { started, finish }: Control,
  count: number,
  completions: Record<string, Partial<OperationCompletion>> = {},
): Promise<void> {
  for (let index = 0; index < count; index++) {
    await settle();
    finish(started[index]!, completions[started[index]!]);
  }
}

const cancelled = { status: "cancelled", error: { message: "killed" }, output: undefined } as const;
const step = (id: string) => shell({ name: id, command: id });
const steps = (count: number) => Array.from({ length: count }, (_, index) => step(`s${index}`));
const ids = (count: number) => steps(count).map((runnable) => runnable.name);
const group = (count: number) =>
  dynamic(() => parallel(steps(count).map((runnable) => runnable())));

const fileJssg = jssg({ name: "file-pass", language: "typescript", transform: ref("file-pass") });
const workspaceJssg = jssg({
  name: "workspace-pass",
  language: "typescript",
  transform: ref("workspace-pass"),
  semanticAnalysis: "workspace",
});

/** Runs `body` under a fresh scheduler and a controllable executor, settled once. */
async function start(
  body: Executable,
  capacity: number,
  options: { signal?: AbortSignal; history?: MemoryHistoryStore } = {},
) {
  const events: RunEvent[] = [];
  const sink = { emit: (event: RunEvent) => events.push(event) };
  const scheduler = new AdmissionScheduler({ capacity });
  const control = controllable();
  const result = run(body, { ...options, executor: control.executor, scheduler, events: sink });
  const outcome = result.then(
    (value) => value,
    (error: unknown) => error,
  );
  await settle();
  return { ...control, control, scheduler, events, result, outcome };
}
const eventsOf = <T extends RunEvent["type"]>(events: RunEvent[], type: T) =>
  events.filter((event): event is Extract<RunEvent, { type: T }> => event.type === type);

describe("operation weights", () => {
  it.each<[string, number, Operation]>([
    ["shell", 1, { kind: "shell", command: "x" }],
    ["agent", 1, { kind: "agent", prompt: "x", backend: { kind: "builtin", tools: [] } }],
    [
      "assessment",
      1,
      {
        kind: "assessment",
        state: "s",
        questions: { ok: { type: "yesno", instructions: "ok?" } },
        provider: "systemone",
        model: "jev-latest",
      },
    ],
    ["jssg without semantics", 2, fileJssg.toOperation(undefined)],
    [
      "jssg with file semantics",
      2,
      { ...fileJssg.toOperation(undefined), semanticAnalysis: "file" },
    ],
    ["jssg with workspace semantics", 4, workspaceJssg.toOperation(undefined)],
    [
      "jssg with a workspace root",
      4,
      { ...fileJssg.toOperation(undefined), semanticAnalysis: { mode: "workspace", root: "src" } },
    ],
  ])("charges %s a weight of %i", (_label, expected, operation) => {
    expect(weightOf(operation)).toBe(expected);
  });
});

describe("default capacity", () => {
  const host = (cpus: number, gibibytes: number): SchedulerHost => ({
    availableParallelism: () => cpus,
    totalmem: () => gibibytes * 1024 * 1024 * 1024,
  });
  const capacityWith = (override: string) =>
    defaultCapacity(DEFAULT_WEIGHTS, host(16, 64), { [CAPACITY_ENV]: override });

  it.each<[string, number, SchedulerHost]>([
    // Plenty of memory: the CPU count decides.
    ["a large host", 16, host(16, 64)],
    ["a laptop", 10, host(10, 32)],
    // A heavy operation is clamped to this capacity and can still run alone.
    ["a tiny host", 1, host(1, 8)],
    ["a three-core host", 3, host(3, 8)],
    // 2 GiB budget / 512 MiB per workspace pass = 4 passes = 16 units, so memory does not bind.
    ["a small-memory host", 8, host(8, 4)],
    // 0.5 GiB budget = 1 pass = 4 units, below the CPU count.
    ["a memory-starved host", 4, host(8, 1)],
  ])("on %s admits %i units", (_label, expected, hostFacts) => {
    expect(defaultCapacity(DEFAULT_WEIGHTS, hostFacts, {})).toBe(expected);
  });

  it("takes the operator override over the host facts", () => {
    expect(capacityWith("3")).toBe(3);
  });

  it("ignores a blank operator override", () => {
    expect(capacityWith("  ")).toBe(16);
  });

  it.each(["0", "-2", "1.5", "many"])("rejects an operator override of %j", (bad) => {
    expect(() => capacityWith(bad)).toThrow(/must be a positive integer/u);
  });

  it("rejects a scheduler capacity of 0", () => {
    expect(() => new AdmissionScheduler({ capacity: 0 })).toThrow(/positive integer/u);
  });
});

describe("bounded admission", () => {
  it("admits a group's members up to capacity and queues the rest", async () => {
    const { started, scheduler } = await start(group(40), 5);
    expect({ started, ...scheduler.stats() }).toMatchObject({
      started: ids(5),
      used: 5,
      active: 5,
      queued: 35,
    });
  });

  it("admits the next member in declaration order when one finishes", async () => {
    const { started, finish } = await start(group(40), 5);
    finish("s2");
    await settle();
    expect(started).toEqual(ids(6));
  });

  it("never exceeds capacity over a whole run", async () => {
    const { control, scheduler, result } = await start(group(40), 5);
    await drain(control, 40);
    await result;
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0, peakActive: 5 });
  });

  describe("with a workspace JSSG pass", () => {
    // Capacity 5 fits one workspace pass (4) plus one shell (1), never two passes.
    const heavy = () =>
      start(
        dynamic(() =>
          parallel(
            workspaceJssg({ id: "w0" }),
            workspaceJssg({ id: "w1" }),
            step("e0")(),
            fileJssg({ id: "f0" }),
          ),
        ),
        5,
      );

    it("keeps lighter work queued behind a pass that does not fit yet", async () => {
      const { started, scheduler } = await heavy();
      expect({ started, ...scheduler.stats() }).toMatchObject({
        started: ["w0"],
        used: 4,
        queued: 3,
      });
    });

    it("admits a shell beside a pass in the capacity left over", async () => {
      const { started, finish, scheduler } = await heavy();
      finish("w0");
      await settle();
      expect({ started, ...scheduler.stats() }).toMatchObject({
        started: ["w0", "w1", "e0"],
        used: 5,
      });
    });

    it("charges the passes their weight over the whole run", async () => {
      const { control, scheduler, result } = await heavy();
      await drain(control, 4, { f0: { output: [] } });
      await result;
      expect(scheduler.stats()).toMatchObject({ used: 0, peakUsed: 5, peakActive: 2 });
    });
  });

  describe("when members finish in reverse order", () => {
    const reversed = async () => {
      const { finish, result } = await start(group(3), 3);
      for (const id of ["s2", "s1", "s0"]) finish(id, { output: { stdout: `done ${id}` } });
      return result;
    };

    it("returns outputs in declaration order", async () => {
      const { output } = await reversed();
      expect((output as { stdout: string }[]).map((o) => o.stdout)).toEqual([
        "done s0",
        "done s1",
        "done s2",
      ]);
    });

    it("records commands in declaration order, so replay is unaffected", async () => {
      const { history } = await reversed();
      const scheduled = history.events.flatMap((event) =>
        event.type === "scheduled" ? [event.command.id] : [],
      );
      expect(scheduled).toEqual(ids(3));
    });
  });

  it("reports commands waiting for capacity as queued without starting them", async () => {
    const { started, events } = await start(group(3), 1);
    const queued = eventsOf(events, "scheduler.queued").map((event) => event.commandId);
    expect([started, queued]).toEqual([["s0"], ["s1", "s2"]]);
  });

  it("records every member as scheduled while it waits for capacity", async () => {
    const { events } = await start(group(3), 1);
    expect(eventsOf(events, "command.scheduled")).toHaveLength(3);
  });

  it("reports each command as admitted once", async () => {
    const { control, events, result } = await start(group(3), 1);
    await drain(control, 3);
    await result;
    expect(eventsOf(events, "scheduler.admitted")).toHaveLength(3);
  });
});

describe("cancellation", () => {
  /** `s0` admitted and `s1` queued under capacity 1, then the run is aborted. */
  const abortedWhileQueued = async () => {
    const controller = new AbortController();
    const history = new MemoryHistoryStore();
    const started = await start(group(2), 1, { signal: controller.signal, history });
    controller.abort();
    await settle();
    return { ...started, history };
  };

  it("refuses a queued command on abort without starting it", async () => {
    const { started, scheduler } = await abortedWhileQueued();
    expect({ started, ...scheduler.stats() }).toMatchObject({ started: ["s0"], queued: 0 });
  });

  it("leaves an admitted command running until its executor reports", async () => {
    const { scheduler } = await abortedWhileQueued();
    expect(scheduler.stats().active).toBe(1);
  });

  it("fails the aborted run with an OperationError", async () => {
    const { finish, outcome } = await abortedWhileQueued();
    finish("s0", cancelled);
    expect(await outcome).toBeInstanceOf(OperationError);
  });

  it("records the refused command as cancelled while queued", async () => {
    const { finish, outcome, history } = await abortedWhileQueued();
    finish("s0", cancelled);
    await outcome;
    const completed = (await history.load()).events.flatMap((event) =>
      event.type === "completed"
        ? [[event.commandId, event.completion.status, event.completion.error?.message]]
        : [],
    );
    expect(completed).toEqual([
      ["s1", "cancelled", "cancelled while queued for execution capacity"],
      ["s0", "cancelled", "killed"],
    ]);
  });

  it("holds no capacity once the aborted run settles", async () => {
    const { finish, outcome, scheduler } = await abortedWhileQueued();
    finish("s0", cancelled);
    await outcome;
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0 });
  });

  describe("when the signal already aborted", () => {
    const alreadyAborted = () => start(step("only")(), 4, { signal: AbortSignal.abort() });

    it("fails the run with an OperationError", async () => {
      const { outcome } = await alreadyAborted();
      expect(await outcome).toBeInstanceOf(OperationError);
    });

    it("admits and starts nothing", async () => {
      const { started, scheduler } = await alreadyAborted();
      expect({ started, ...scheduler.stats() }).toMatchObject({ started: [], peakActive: 0 });
    });
  });
});

describe("permits", () => {
  it.each<[string, (finish: Control["finish"], abort: () => void) => void]>([
    ["success", (finish) => finish("only")],
    [
      "an operation failure",
      (finish) =>
        finish("only", { status: "failed", error: { message: "boom" }, output: undefined }),
    ],
    [
      "cancellation",
      (finish, abort) => {
        abort();
        finish("only", cancelled);
      },
    ],
  ])("are released after %s", async (_label, end) => {
    const controller = new AbortController();
    const { finish, outcome, scheduler } = await start(step("only")(), 2, {
      signal: controller.signal,
    });
    end(finish, () => controller.abort());
    await outcome;
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0, peakActive: 1 });
  });

  it("are released after an executor launch failure", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 2 });
    const { executor } = controllable(["only"]);
    await run(step("only")(), { executor, scheduler }).catch(() => {});
    expect(scheduler.stats()).toMatchObject({ used: 0, active: 0, queued: 0, peakActive: 1 });
  });
});

describe("replay", () => {
  const replayUnderCapacityOne = async () => {
    const body = group(4);
    const first = createHarness({ fallback: (request) => `ran ${request.commandId}` });
    const recorded = await first.run(body);
    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const replay = createHarness({
      history: first.serialize(),
      fallback: () => failed("must not execute"),
      scheduler,
    });
    return { recorded, replay, replayed: await replay.run(body), scheduler };
  };

  it("replays a recorded group without reaching the executor", async () => {
    const { recorded, replay, replayed } = await replayUnderCapacityOne();
    expect([replayed.replayed, replayed.output, replay.executed]).toEqual([
      true,
      recorded.output,
      [],
    ]);
  });

  it("consumes no capacity while replaying", async () => {
    const { scheduler } = await replayUnderCapacityOne();
    expect(scheduler.stats()).toMatchObject({ peakActive: 0, peakUsed: 0, used: 0 });
  });
});

describe("group shapes", () => {
  it("bounds two groups awaited at once, one built inside the body, by the one run scheduler", async () => {
    const late = steps(3).map((runnable, index) => runnable({ id: `late${index}` }));
    const body = dynamic(() =>
      Promise.all([
        parallel(steps(3).map((runnable, index) => runnable({ id: `early${index}` }))),
        parallel(late),
      ]),
    );
    const { control, started, scheduler, result } = await start(body, 2);
    await drain(control, 6);
    await result;
    expect({ started: started.length, ...scheduler.stats() }).toMatchObject({
      started: 6,
      peakActive: 2,
      used: 0,
    });
  });

  it("runs a sequence's parallel stage under the bound, before the next stage", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 2 });
    const harness = createHarness({ fallback: (request) => `ran ${request.commandId}`, scheduler });
    const result = await harness.run(
      sequence(parallel(steps(3).map((runnable) => runnable())), step("last")()),
    );
    expect([result.commands.map((command) => command.id), scheduler.stats().peakActive]).toEqual([
      [...ids(3), "last"],
      2,
    ]);
  });

  it("does not change a sequential workflow at capacity 1", async () => {
    const scheduler = new AdmissionScheduler({ capacity: 1 });
    const [first, second] = [step("first"), step("second")];
    const harness = createHarness({ fallback: (request) => `ran ${request.commandId}`, scheduler });
    const result = await harness.run(
      dynamic(async () => [(await first()).stdout, (await second()).stdout]),
    );
    expect([result.output, scheduler.stats().peakActive]).toEqual([["ran first", "ran second"], 1]);
  });

  it("bounds two concurrent runs by their own schedulers", async () => {
    const [narrow, wide] = await Promise.all([start(group(3), 1), start(group(3), 3)]);
    expect([narrow.scheduler.stats(), wide.scheduler.stats()]).toMatchObject([
      { active: 1, queued: 2 },
      { active: 3, queued: 0 },
    ]);
  });
});

import { describe, expect, it } from "vitest";
import { createHarness, failed } from "../src/host/harness.ts";
import {
  CompositionValidationError,
  DuplicateCommandIdError,
  NoActiveRunError,
  shell,
  executableRequiresInput,
  guard,
  isExecutable,
  parallel,
  sequence,
  dynamic,
  type ShellOutput,
} from "../src/index.ts";

const Count = guard(
  "Count",
  (value: unknown): value is { count: number } =>
    typeof value === "object" &&
    value !== null &&
    typeof (value as { count?: unknown }).count === "number",
);
const Text = guard("Text", (value: unknown): value is string => typeof value === "string");

const seed = shell({ name: "seed", command: "seed", output: Count });
const increment = shell({
  name: "increment",
  input: Count,
  output: Count,
  command: ({ count }) => `increment ${count}`,
});
const render = shell({
  name: "render",
  input: Count,
  output: Text,
  command: ({ count }) => `render ${count}`,
});
const sideEffect = shell({ name: "side-effect", command: "side-effect" });

describe("static composition", () => {
  it("flows values through a sequence and returns only the final output", async () => {
    const definition = sequence(seed(), increment(), render());
    const h = createHarness({
      results: { seed: { count: 1 }, increment: { count: 2 }, render: '"count=2"' },
    });

    const result = await h.run(definition);
    expect(result.output).toBe("count=2");
    expect(result.commands.map(({ id, input }) => [id, input])).toEqual([
      ["seed", undefined],
      ["increment", { count: 1 }],
      ["render", { count: 2 }],
    ]);
    const typed: string = result.output;
    expect(typed).toBe("count=2");
  });

  it("lets metadata-only invocation options preserve no-input mode", async () => {
    const h = createHarness({ results: { seed: { count: 3 }, "side:bound": "done" } });
    const result = await h.run(sequence(seed(), sideEffect({ id: "side:bound" })));

    expect(result.output).toEqual({ stdout: "done" });
    expect(result.commands[1]).not.toHaveProperty("input");
  });

  it("preserves flow when invocation options only add metadata", async () => {
    const h = createHarness({
      results: { seed: { count: 3 }, "increment:flow": { count: 4 } },
    });
    const result = await h.run(sequence(seed(), increment({ id: "increment:flow" })));

    expect(result.output).toEqual({ count: 4 });
    expect(result.commands[1]?.input).toEqual({ count: 3 });
  });

  it("lets explicitly bound input ignore the previous output", async () => {
    const h = createHarness({
      results: { seed: { count: 3 }, "increment:bound": { count: 11 } },
    });
    const result = await h.run(
      sequence(seed(), increment({ id: "increment:bound", input: { count: 10 } })),
    );

    expect(result.output).toEqual({ count: 11 });
    expect(result.commands[1]?.input).toEqual({ count: 10 });
  });

  it("lets a no-input invocation ignore the previous output", async () => {
    const h = createHarness({ results: { seed: { count: 3 }, "side-effect": "done" } });
    const result = await h.run(sequence(seed(), sideEffect()));

    expect(result.output).toEqual({ stdout: "done" });
    expect(result.commands[1]).not.toHaveProperty("input");
  });

  it("uses an explicit workflow for synchronous data computation", async () => {
    const aggregate = dynamic((value: { count: number }) => ({ total: value.count * 3 }));
    const result = await createHarness({ results: { seed: { count: 4 } } }).run(
      sequence(seed(), aggregate),
    );

    expect(result.output).toEqual({ total: 12 });
    expect(result.commands).toHaveLength(1);
  });

  it("fans one value into parallel members and rejoins outputs as a tuple", async () => {
    const stringify = dynamic((value: { count: number }) => `value:${value.count}`);
    const result = await createHarness({
      results: { seed: { count: 5 }, increment: { count: 6 } },
    }).run(sequence(seed(), parallel(increment(), stringify)));

    expect(result.output).toEqual([{ count: 6 }, "value:5"]);
    const typed: [{ count: number }, string] = result.output;
    expect(typed[0].count).toBe(6);
  });

  it("composes parallel sequences and preserves member order", async () => {
    const left = sequence(
      dynamic((value: { count: number }) => ({ count: value.count + 1 })),
      dynamic((value: { count: number }) => `left:${value.count}`),
    );
    const right = sequence(
      dynamic(async (value: { count: number }) => ({ count: value.count + 2 })),
      dynamic((value: { count: number }) => `right:${value.count}`),
    );
    const result = await createHarness({ results: { seed: { count: 7 } } }).run(
      sequence(seed(), parallel(left, right)),
    );

    expect(result.output).toEqual(["left:8", "right:9"]);
  });

  it("supports sequence-parallel-sequence nesting at several levels", async () => {
    const branch = (amount: number) =>
      sequence(
        dynamic((value: number) => value + amount),
        parallel(
          dynamic((value: number) => value * 2),
          sequence(
            dynamic((value: number) => value * 3),
            dynamic((value: number) => `${value}`),
          ),
        ),
        dynamic(([number, text]: [number, string]) => `${number}:${text}`),
      );
    const definition = sequence(
      dynamic(() => 1),
      parallel(branch(1), branch(2), branch(3)),
      dynamic((values: [string, string, string]) => values.join("|")),
    );

    expect((await createHarness().run(definition)).output).toBe("4:6|6:9|8:12");
  });

  it("passes input through directly nested parallel groups", async () => {
    const definition = sequence(
      dynamic(() => 3),
      parallel(
        parallel(
          dynamic((value: number) => value + 1),
          dynamic((value: number) => value + 2),
        ),
        dynamic((value: number) => value + 3),
      ),
    );

    expect((await createHarness().run(definition)).output).toEqual([[4, 5], 6]);
  });

  it("matches equivalent sequencing and concurrency authored in workflows", async () => {
    const staticSequence = sequence(seed(), increment());
    const dynamicSequence = dynamic(async () => increment({ input: await seed() }));
    const staticParallel = parallel(seed({ id: "seed:a" }), seed({ id: "seed:b" }));
    const dynamicParallel = dynamic(() =>
      Promise.all([seed({ id: "seed:a" }), seed({ id: "seed:b" })]),
    );
    const results = {
      seed: { count: 1 },
      increment: { count: 2 },
      "seed:a": { count: 3 },
      "seed:b": { count: 4 },
    };

    expect((await createHarness({ results }).run(staticSequence)).output).toEqual(
      (await createHarness({ results }).run(dynamicSequence)).output,
    );
    expect((await createHarness({ results }).run(staticParallel)).output).toEqual(
      (await createHarness({ results }).run(dynamicParallel)).output,
    );
  });

  it("allows workflows to issue nested static compositions", async () => {
    const definition = dynamic(async () => {
      const first = await sequence(seed(), increment());
      const rest = await parallel(
        dynamic(() => first.count),
        sequence(
          dynamic(() => first),
          render(),
        ),
      );
      return rest;
    });
    const result = await createHarness({
      results: { seed: { count: 2 }, increment: { count: 3 }, render: '"three"' },
    }).run(definition);

    expect(result.output).toEqual([3, "three"]);
    expect(result.commands.map((command) => command.id)).toEqual(["seed", "increment", "render"]);
  });

  it("allows a workflow stage to retain intermediate state explicitly", async () => {
    const retain = dynamic(async (initial: { count: number }) => ({
      initial,
      updated: await increment({ input: initial }),
    }));
    const definition = sequence(
      seed(),
      retain,
      dynamic(
        ({ initial, updated }: { initial: { count: number }; updated: { count: number } }) =>
          initial.count + updated.count,
      ),
    );
    const result = await createHarness({
      results: { seed: { count: 2 }, increment: { count: 3 } },
    }).run(definition);

    expect(result.output).toBe(5);
  });

  it("can run a parallel group as the root executable", async () => {
    const first = shell({ name: "first", command: "first" });
    const second = shell({ name: "second", command: "second" });
    const result = await createHarness({ results: { first: "a", second: "b" } }).run(
      parallel(first(), second()),
    );
    expect(result.output).toEqual([{ stdout: "a" }, { stdout: "b" }]);
  });

  it("snapshots members supplied as an array", async () => {
    const first = shell({ name: "first", command: "first" });
    const second = shell({ name: "second", command: "second" });
    const members = [first()];
    const definition = parallel(members);
    members.push(second());

    const result = await createHarness({ results: { first: "a" } }).run(definition);
    expect(result.output).toEqual([{ stdout: "a" }]);
    expect(definition.ir.root.members).toHaveLength(1);
  });

  it("stops a sequence after a failed stage", async () => {
    const h = createHarness({ results: { seed: { count: 1 }, increment: failed("nope") } });
    await expect(h.run(sequence(seed(), increment(), render()))).rejects.toThrow("nope");
    expect(h.executed.map((request) => request.commandId)).toEqual(["seed", "increment"]);
  });

  it("allows recovery from a failed sequence without treating later stages as forgotten", async () => {
    const definition = dynamic(async () => {
      try {
        await sequence(sideEffect({ id: "fails" }), sideEffect({ id: "must-not-run" }));
      } catch {
        return "recovered";
      }
      return "unexpected";
    });
    const harness = createHarness({ results: { fails: failed("nope") } });

    expect((await harness.run(definition)).output).toBe("recovered");
    expect(harness.executed.map((request) => request.commandId)).toEqual(["fails"]);
  });

  it("replays nested composition without executing and checks final output", async () => {
    const definition = sequence(
      parallel(
        seed(),
        dynamic(() => "constant"),
      ),
      dynamic(([value, label]: [{ count: number }, string]) => `${label}:${value.count}`),
    );
    const first = createHarness({ results: { seed: { count: 9 } } });
    expect((await first.run(definition)).output).toBe("constant:9");

    const replay = first.reload({ fallback: () => failed("must not execute") });
    expect((await replay.run(definition)).output).toBe("constant:9");
    expect(replay.executed).toHaveLength(0);
  });

  it("replays parallel sequences when later stages were issued in completion order", async () => {
    const a1 = shell({ name: "a1", command: "a1" });
    const a2 = shell({ name: "a2", command: "a2" });
    const b1 = shell({ name: "b1", command: "b1" });
    const b2 = shell({ name: "b2", command: "b2" });
    let releaseA1!: () => void;
    const first = createHarness({
      results: {
        a1: () =>
          new Promise<string>((resolve) => {
            releaseA1 = () => resolve("a1");
          }),
        a2: "a2",
        b1: "b1",
        b2: () => {
          releaseA1();
          return "b2";
        },
      },
    });
    const definition = parallel(sequence(a1(), a2()), sequence(b1(), b2()));
    const recorded = await first.run(definition);

    expect(recorded.commands.map((command) => command.id)).toEqual(["a1", "b1", "b2", "a2"]);
    expect(recorded.commands.every((command) => command.concurrent === true)).toBe(true);
    const replay = first.reload({ fallback: () => failed("must not execute") });
    expect((await replay.run(definition)).output).toEqual(recorded.output);
    expect(replay.executed).toHaveLength(0);
  });

  it("reports only the missing member when replay removes an earlier parallel command", async () => {
    const a = shell({ name: "a", command: "a" });
    const b = shell({ name: "b", command: "b" });
    const first = createHarness({ results: { a: "a", b: "b" } });
    await first.run(parallel(a(), b()));

    await expect(
      first.reload({ fallback: () => failed("must not execute") }).run(parallel(b())),
    ).rejects.toMatchObject({ name: "NondeterminismError", detail: { missing: ["a"] } });
  });

  it("waits for sibling branches after a parallel member fails", async () => {
    const fails = shell({ name: "fails", command: "fails" });
    const slow = shell({ name: "slow", command: "slow" });
    const after = shell({ name: "after", command: "after" });
    let releaseSlow!: () => void;
    const harness = createHarness({
      results: {
        fails: failed("nope"),
        slow: () =>
          new Promise<string>((resolve) => {
            releaseSlow = () => resolve("slow");
          }),
        after: "after",
      },
    });
    const attempt = harness.run(parallel(fails(), sequence(slow(), after())));
    let settled = false;
    void attempt.then(
      () => {
        settled = true;
      },
      () => {
        settled = true;
      },
    );

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(settled).toBe(false);
    expect(harness.executed.map((request) => request.commandId)).toEqual(["fails", "slow"]);
    releaseSlow();
    await expect(attempt).rejects.toThrow("nope");
    expect(harness.executed.map((request) => request.commandId)).toEqual([
      "fails",
      "slow",
      "after",
    ]);
  });

  it("detects duplicate operation identities across nested branches before execution", () => {
    expect(() =>
      parallel(
        sideEffect(),
        sequence(
          dynamic(() => undefined),
          sideEffect(),
        ),
      ),
    ).toThrow(DuplicateCommandIdError);
  });

  it("builds nested static IR and leaves workflow code opaque", () => {
    const definition = sequence(
      seed(),
      parallel(increment(), sideEffect({ id: "bound" })),
      dynamic(() => "done"),
    );

    expect(definition.ir).toEqual({
      version: 1,
      root: {
        type: "sequence",
        stages: [
          { type: "operation", id: "seed", name: "seed", kind: "shell", input: "none" },
          {
            type: "parallel",
            members: [
              {
                type: "operation",
                id: "increment",
                name: "increment",
                kind: "shell",
                input: "flow",
              },
              {
                type: "operation",
                id: "bound",
                name: "side-effect",
                kind: "shell",
                input: "none",
              },
            ],
          },
          { type: "dynamic" },
        ],
      },
    });
  });

  it("rejects empty and non-stage compositions", () => {
    // @ts-expect-error an empty sequence is rejected by both the types and runtime
    expect(() => sequence()).toThrow(CompositionValidationError);
    expect(() => parallel()).toThrow(CompositionValidationError);
    expect(() => parallel([])).toThrow(CompositionValidationError);
    expect(() =>
      sequence(seed(), ((value: unknown) => value) as unknown as ReturnType<typeof sideEffect>),
    ).toThrow(CompositionValidationError);
    // @ts-expect-error raw functions must be wrapped with dynamic()
    const _rawFunction = () => sequence(seed(), (value: { count: number }) => value.count);
    // @ts-expect-error bare runnable definitions are only accepted as root executables
    const _bareDefinition = () => sequence(seed);
    // @ts-expect-error a root sequence that requires input needs run(..., { input })
    const _requiredRoot = () => createHarness().run(sequence(increment(), render));
    // @ts-expect-error a bound member does not erase another parallel member's required input
    const _mixedRequiredRoot = () => createHarness().run(parallel(sideEffect(), increment()));
    const _workflowMixedRequiredRoot = () =>
      // @ts-expect-error a no-argument workflow is neutral to a required parallel member
      createHarness().run(
        parallel(
          dynamic(() => "constant"),
          increment(),
        ),
      );
    const requiredMembers = [increment()];
    // @ts-expect-error array groups retain their members' required input
    const _arrayRequiredRoot = () => createHarness().run(parallel(requiredMembers));
    const _wrongRootInput = () =>
      // @ts-expect-error the root input must match the first stage's input type
      createHarness().run(sequence(increment(), render()), { input: { count: "one" } });
    const _requiredInputIsNotAwaitable = () =>
      dynamic(async () => {
        // @ts-expect-error a required-input node must receive its value from static composition
        const output: string = await sequence(increment(), render());
        return output;
      });
    // Required roots run once their input is supplied; optional roots may omit or pass it.
    const _suppliedRoot = () =>
      createHarness().run(sequence(increment(), render()), { input: { count: 1 } });
    const _optionalRoot = () =>
      createHarness().run(sequence(seed(), render()), { input: undefined });
    expect(_rawFunction).toBeTypeOf("function");
    expect(_bareDefinition).toBeTypeOf("function");
    expect(_requiredRoot).toBeTypeOf("function");
    expect(_mixedRequiredRoot).toBeTypeOf("function");
    expect(_workflowMixedRequiredRoot).toBeTypeOf("function");
    expect(_arrayRequiredRoot).toBeTypeOf("function");
    expect(_wrongRootInput).toBeTypeOf("function");
    expect(_requiredInputIsNotAwaitable).toBeTypeOf("function");
    expect(_suppliedRoot).toBeTypeOf("function");
    expect(_optionalRoot).toBeTypeOf("function");
  });

  it("passes a root input into a required-input sequence, parallel group, and runnable", async () => {
    const h = createHarness({ results: { increment: { count: 2 }, render: '"count=2"' } });
    const result = await h.run(sequence(increment(), render()), { input: { count: 1 } });
    expect(result.output).toBe("count=2");
    expect(result.commands.map(({ id, input }) => [id, input])).toEqual([
      ["increment", { count: 1 }],
      ["render", { count: 2 }],
    ]);

    const fanned = await createHarness({ results: { increment: { count: 4 } } }).run(
      parallel(
        increment(),
        dynamic((value: { count: number }) => value.count * 10),
      ),
      { input: { count: 3 } },
    );
    expect(fanned.output).toEqual([{ count: 4 }, 30]);

    const direct = await createHarness({ results: { render: '"count=5"' } }).run(render(), {
      input: { count: 5 },
    });
    expect(direct.output).toBe("count=5");
    expect(direct.commands[0]?.input).toEqual({ count: 5 });
  });

  it("runs a bare runnable as the root and validates a missing required input", async () => {
    const result = await createHarness({ results: { seed: { count: 8 } } }).run(seed);
    expect(result.output).toEqual({ count: 8 });
    expect(result.commands).toEqual([
      {
        id: "seed",
        runnable: "seed",
        kind: "shell",
        operation: { kind: "shell", command: "seed" },
      },
    ]);
    expect(isExecutable(seed)).toBe(true);
    expect(isExecutable(seed())).toBe(true);
    expect(isExecutable(() => undefined)).toBe(false);
    expect(executableRequiresInput(seed())).toBe(false);
    expect(executableRequiresInput(increment)).toBe(true);
    expect(executableRequiresInput(increment())).toBe(true);
    expect(executableRequiresInput(sequence(increment(), render()))).toBe(true);
    expect(executableRequiresInput(sequence(seed(), increment()))).toBe(false);
    expect(executableRequiresInput(parallel(seed(), increment()))).toBe(true);
    expect(executableRequiresInput(increment({ input: { count: 1 } }))).toBe(false);
    expect(executableRequiresInput(dynamic((value: number) => value))).toBe(true);
    expect(executableRequiresInput(dynamic(() => 1))).toBe(false);

    const h = createHarness({ fallback: () => ({ count: 0 }) });
    await expect(h.run(increment, { input: null as unknown as { count: number } })).rejects.toThrow(
      /input of 'increment': expected Count/u,
    );
    expect(h.executed).toHaveLength(0);
  });

  it("rejects composition awaited outside an active workflow", async () => {
    await expect(sequence(seed())).rejects.toBeInstanceOf(NoActiveRunError);
    await expect(parallel(seed())).rejects.toBeInstanceOf(NoActiveRunError);
  });

  it("refuses to finalize an unawaited static node built inside a workflow", async () => {
    const definition = dynamic(() => {
      sequence(seed(), increment());
      return "done";
    });
    const harness = createHarness({ results: { seed: { count: 1 }, increment: { count: 2 } } });

    await expect(harness.run(definition)).rejects.toThrow(
      "workflow body returned without awaiting 2 operation(s): seed, increment",
    );
    expect(harness.executed).toHaveLength(0);
  });

  it("refuses an unawaited static node containing only opaque workflows", async () => {
    const definition = dynamic(() => {
      sequence(dynamic(() => "ignored"));
      return "done";
    });

    await expect(createHarness().run(definition)).rejects.toThrow(
      "workflow body returned without awaiting 1 operation(s): sequence (opaque)",
    );
  });

  it("keeps command outputs typed when commands ignore flowing values", async () => {
    const definition = sequence(seed(), sideEffect());
    const result = await createHarness({
      results: { seed: { count: 1 }, "side-effect": "ok" },
    }).run(definition);
    const output: ShellOutput = result.output;
    expect(output.stdout).toBe("ok");
  });
});

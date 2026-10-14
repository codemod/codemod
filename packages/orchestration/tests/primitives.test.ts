/** Author-facing shell and dynamic primitives. */
import { describe, expect, it } from "vitest";
import { createHarness } from "../src/host/harness.ts";
import {
  dynamic,
  guard,
  isExecutable,
  parallel,
  sequence,
  shell,
  type Command,
  type Dynamic,
  type ShellOutput,
  type ShellRunnable,
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
const render = shell({
  name: "render",
  input: Count,
  output: Text,
  command: ({ count }) => `render ${count}`,
});
const list = shell({ name: "list", command: "ls" });

describe("shell", () => {
  it("uses the shell wire kind", () => {
    expect(seed.kind).toBe("shell");
    expect(seed.toOperation(undefined)).toEqual({ kind: "shell", command: "seed" });
    expect(render.toOperation({ count: 3 })).toEqual({ kind: "shell", command: "render 3" });
    expect(
      sequence(seed(), render()).ir.root.stages.map(
        (stage) => stage.type === "operation" && stage.kind,
      ),
    ).toEqual(["shell", "shell"]);
    expect(isExecutable(seed())).toBe(true);
  });

  it("preserves the generic typing of definitions, invocations, and outputs", () => {
    const _seed: ShellRunnable<void, { count: number }> = seed;
    const _render: ShellRunnable<{ count: number }, string> = render;
    const _list: ShellRunnable = list;
    const _bound: Command<string> = render({ input: { count: 1 } });
    const _raw: Command<ShellOutput> = list();
    const _same: ShellOutput = { stdout: "" } satisfies ShellOutput;
    const _flow: Command<string, { count: number }> = render();
    // @ts-expect-error shell steps never take a target
    const _target = () => list({ target: { root: "src" } });
    // @ts-expect-error seed yields Count, list takes nothing but render needs Count
    const _incompatible = () => sequence(list(), render());
    expect([_seed, _render, _list, _bound, _raw, _same, _flow]).toHaveLength(7);
    expect([_target, _incompatible]).toHaveLength(2);
  });

  it("runs and decodes like shell", async () => {
    const h = createHarness({ results: { seed: { count: 2 }, render: '"two"', list: "a\nb\n" } });
    const result = await h.run(sequence(seed(), render()));
    const output: string = result.output;
    expect(output).toBe("two");
    expect(result.commands.map(({ id, kind, input }) => [id, kind, input])).toEqual([
      ["seed", "shell", undefined],
      ["render", "shell", { count: 2 }],
    ]);
    const raw = await createHarness({ results: { list: "a\nb\n" } }).run(list());
    expect(raw.output).toEqual({ stdout: "a\nb\n" });
  });
});

describe("dynamic", () => {
  it("creates an opaque dynamic node", () => {
    const node = dynamic(() => "done");
    expect(node.type).toBe("dynamic");
    expect(isExecutable(node)).toBe(true);
    expect(sequence(seed(), node).ir.root.stages[1]).toEqual({ type: "dynamic" });
  });

  it("preserves both overloads and the stage typing", () => {
    const noInput = dynamic(() => 1);
    const withInput = dynamic((value: { count: number }) => value.count * 2);
    const _noInput: Dynamic<void, number> = noInput;
    const _withInput: Dynamic<{ count: number }, number> = withInput;
    const _async: Dynamic<{ count: number }, string> = dynamic(async (value: { count: number }) =>
      String(value.count),
    );
    // @ts-expect-error a dynamic step's parameter must match the preceding output
    const _mismatch = () => sequence(render(), withInput);
    // @ts-expect-error a required-input dynamic root needs run(..., { input })
    const _requiredRoot = () => createHarness().run(withInput);
    expect([_noInput, _withInput, _async]).toHaveLength(3);
    expect([_mismatch, _requiredRoot]).toHaveLength(2);
  });

  it("flows typed data through static composition and awaits commands inside its body", async () => {
    const double = dynamic((value: { count: number }) => ({ count: value.count * 2 }));
    const branch = dynamic(async (value: { count: number }) => {
      if (value.count > 5) return "large";
      const [text, listing] = await parallel(render({ input: value }), list());
      return `${text}:${listing.stdout.trim()}`;
    });
    const h = createHarness({ results: { seed: { count: 2 }, render: '"four"', list: "x\n" } });
    const result = await h.run(sequence(seed(), double, branch));
    const output: string = result.output;
    expect(output).toBe("four:x");
    // The group's members are concurrent, so history records them in completion order.
    const ids = result.commands.map((command) => command.id);
    expect(ids[0]).toBe("seed");
    expect(ids.slice(1).sort()).toEqual(["list", "render"]);
    expect(result.commands.slice(1).every((command) => command.concurrent === true)).toBe(true);

    const direct = await createHarness().run(double, { input: { count: 21 } });
    expect(direct.output).toEqual({ count: 42 });
  });
});

/**
 * The callable runtime: calling a runnable creates a lazy command, awaiting it
 * inside a workflow issues it, and static composition nodes are awaitable data.
 */
import { describe, expect, it } from "vitest";
import { createHarness, failed } from "../src/host/harness.ts";
import {
  DuplicateCommandIdError,
  PROTOCOL_VERSION,
  ai,
  shell,
  guard,
  isCommand,
  jssg,
  parallel,
  run,
  sequence,
  dynamic,
  type Command,
  type Json,
  type OperationCompletion,
  type OperationRequest,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

interface Package {
  name: string;
  path: string;
}
interface Report {
  package: string;
  findings: string[];
}

const Packages = guard("Packages", (v: unknown): v is Package[] => Array.isArray(v));
const Package = guard(
  "Package",
  (v: unknown): v is Package => typeof v === "object" && v !== null && "path" in v,
);
const Report = guard(
  "Report",
  (v: unknown): v is Report => typeof v === "object" && v !== null && "findings" in v,
);
const Summary = guard(
  "Summary",
  (v: unknown): v is { total: number } => typeof v === "object" && v !== null,
);

const discover = shell({ name: "discover", command: "node discover.js", output: Packages });
const inspectPackage = shell({
  name: "inspect-package",
  input: Package,
  output: Report,
  command: "node inspect-package.js",
  env: (pkg) => ({ PACKAGE_PATH: pkg.path }),
});
const writeReport = jssg({
  name: "write-report",
  transform: ref("write-report"),
  language: "typescript",
  input: guard("Findings", (v: unknown): v is string[] => Array.isArray(v)),
  output: Summary,
});
const format = shell({ name: "format", command: "npm run format" });
const lint = shell({ name: "lint", command: "lint" });

const packages: Package[] = [
  { name: "a", path: "packages/a" },
  { name: "b", path: "packages/b" },
  { name: "c", path: "packages/c" },
];

const audit = dynamic(async () => {
  const found = await discover();
  const reports = await parallel(
    found.map((pkg) => inspectPackage({ input: pkg, id: `inspect:${pkg.name}` })),
  );
  const findings = reports.flatMap((report) => report.findings);
  return writeReport({ input: findings });
});

const auditResults: Record<string, Json> = {
  discover: packages.map((pkg) => ({ ...pkg })),
  "inspect:a": { package: "a", findings: ["a1", "a2"] },
  "inspect:b": { package: "b", findings: [] },
  "inspect:c": { package: "c", findings: ["c1"] },
  "write-report": { total: 3 },
};

describe("commands", () => {
  it("are created by calling a runnable and carry id, input, and target as data", () => {
    const bare = format();
    expect(isCommand(bare)).toBe(true);
    expect(bare.type).toBe("command");
    expect(bare.id).toBe("format");
    expect(bare.runnable).toBe(format);
    expect(bare.inputMode).toBe("none");
    expect(bare.input).toBeUndefined();
    expect(bare).not.toHaveProperty("target");

    const bound = writeReport({ input: ["x"], id: "report:x", target: { root: "docs" } });
    expect(bound.id).toBe("report:x");
    expect(bound.inputMode).toBe("bound");
    expect(bound.input).toEqual(["x"]);
    expect(bound.target).toEqual({ root: "docs" });
    expect(isCommand(format)).toBe(false);
    expect(isCommand({ type: "command" })).toBe(false);
  });

  it("type-checks invocation options per runnable kind", () => {
    const summarize = ai({ name: "summarize", prompt: "summarize", input: Report });
    const _ok: Command<Report> = inspectPackage({ input: packages[0]! });
    const _okAi: Command<unknown> = summarize({ input: { package: "a", findings: [] }, id: "s" });
    const _okJssg: Command<{ total: number }> = writeReport({ input: [], target: { root: "a" } });
    const _okVoid: Command<{ stdout: string }> = format({ id: "format:2" });
    const _flow: Command<Report, Package> = inspectPackage();
    const _flowAi: Command<unknown, Report> = summarize({ id: "s" });
    // @ts-expect-error shell does not take a target
    const _execTarget = () => inspectPackage({ input: packages[0]!, target: { root: "a" } });
    // @ts-expect-error ai does not take a target
    const _aiTarget = () => summarize({ input: packages[0]!, target: { root: "a" } });
    // @ts-expect-error discover returns Package[], but inspectPackage consumes one Package
    const _incompatibleSequence = () => sequence(discover, inspectPackage);
    expect([_ok, _okAi, _okJssg, _okVoid, _flow, _flowAi]).toHaveLength(6);
  });

  it("rejects a flow invocation awaited without static composition", async () => {
    const definition = dynamic(() => inspectPackage());
    await expect(createHarness().run(definition)).rejects.toThrow(
      "requires flowing input; place it in sequence()/parallel() or invoke it with { input }",
    );
  });

  it("do nothing until awaited inside a workflow", async () => {
    const h = createHarness({ fallback: () => "ok" });
    const created: Command[] = [];
    const wf = dynamic(async () => {
      const first = lint({ id: "lint:1" });
      const second = lint({ id: "lint:2" });
      created.push(first, second);
      expect(h.executed).toHaveLength(0);
      await second;
      expect(h.executed.map((r) => r.commandId)).toEqual(["lint:2"]);
      await first;
      return "done";
    });
    const result = await h.run(wf);
    expect(result.commands.map((c) => c.id)).toEqual(["lint:2", "lint:1"]);
    expect(created).toHaveLength(2);
  });

  it("issue once per run no matter how often they are awaited", async () => {
    const h = createHarness({ fallback: (r) => `ran ${r.commandId}` });
    const wf = dynamic(async () => {
      const command = lint();
      const [a, b] = await Promise.all([command, command]);
      const c = await command;
      return [a.stdout, b.stdout, c.stdout];
    });
    const result = await h.run(wf);
    expect(result.output).toEqual(["ran lint", "ran lint", "ran lint"]);
    expect(result.commands).toHaveLength(1);
    expect(h.executed).toHaveLength(1);
  });

  it("created at module level can be awaited in several runs and replay per run", async () => {
    const shared = lint({ id: "lint:shared" });
    const wf = dynamic(async () => (await shared).stdout);
    const h = createHarness({ fallback: () => "first" });
    const first = await h.run(wf);
    expect(first.output).toBe("first");

    const replay = await h.reload({ fallback: () => failed("must not execute") }).run(wf);
    expect(replay.replayed).toBe(true);
    expect(replay.output).toBe("first");

    const fresh = await createHarness({ fallback: () => "second" }).run(wf);
    expect(fresh.output).toBe("second");
    expect(fresh.replayed).toBe(false);
  });

  it("can be returned from the body and are executed before finalization", async () => {
    const h = createHarness({ results: auditResults });
    const result = await h.run(audit);
    expect(result.output).toEqual({ total: 3 });
    expect(result.commands.map((c) => c.id)).toEqual([
      "discover",
      "inspect:a",
      "inspect:b",
      "inspect:c",
      "write-report",
    ]);
    expect(result.commands[1]?.operation).toEqual({
      kind: "shell",
      command: "node inspect-package.js",
      env: { PACKAGE_PATH: "packages/a" },
    });
    expect(result.commands[4]?.input).toEqual(["a1", "a2", "c1"]);
    expect(result.history.events.at(-1)).toEqual({ type: "finalized", output: { total: 3 } });
  });

  it("validate input against the runnable schema when issued, not when created", async () => {
    const h = createHarness({ fallback: () => ({}) });
    const bad = inspectPackage({ input: { name: "x" } as unknown as Package });
    expect(bad.input).toEqual({ name: "x" });
    await expect(h.run(dynamic(() => bad))).rejects.toThrow(
      /input of 'inspect-package': expected Package/,
    );
    expect(h.executed).toHaveLength(0);
  });
});

describe("dynamic parallel groups", () => {
  it("record members in declaration order and return outputs in that order", async () => {
    const order: string[] = [];
    const executor = {
      async execute(request: OperationRequest): Promise<OperationCompletion> {
        // Finish in reverse order to show outputs are not completion-ordered.
        const delay = { "inspect:a": 30, "inspect:b": 20, "inspect:c": 10 }[request.commandId] ?? 0;
        await new Promise((resolve) => setTimeout(resolve, delay));
        order.push(request.commandId);
        const value = auditResults[request.commandId] ?? null;
        const output: Json =
          request.operation.kind === "shell" ? { stdout: JSON.stringify(value) } : value;
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "succeeded",
          output,
        };
      },
    };
    const perPackage = dynamic(async () => {
      const found = await discover();
      const reports = await parallel(
        found.map((pkg) => inspectPackage({ input: pkg, id: `inspect:${pkg.name}` })),
      );
      return reports.map((report) => report.package);
    });
    const result = await run(perPackage, { executor });
    expect(order).toEqual(["discover", "inspect:c", "inspect:b", "inspect:a"]);
    expect(result.output).toEqual(["a", "b", "c"]);
    expect(
      result.history.events.flatMap((e) => (e.type === "scheduled" ? [e.command.id] : [])),
    ).toEqual(["discover", "inspect:a", "inspect:b", "inspect:c"]);
  });

  it("replay the whole group and detect a changed member", async () => {
    const h = createHarness({ results: auditResults });
    const first = await h.run(audit);
    const replay = await h.reload({ fallback: () => failed("must not execute") }).run(audit);
    expect(replay.replayed).toBe(true);
    expect(replay.output).toEqual(first.output);

    const renamed = dynamic(async () => {
      const found = await discover();
      return parallel(
        found.map((pkg) =>
          inspectPackage({ input: { ...pkg, path: `${pkg.path}/src` }, id: `inspect:${pkg.name}` }),
        ),
      );
    });
    await expect(
      h.reload({ fallback: () => failed("must not execute") }).run(renamed),
    ).rejects.toMatchObject({ name: "NondeterminismError", kind: "changed" });
  });

  it("rejects duplicate ids in a dynamic group before anything executes", async () => {
    let executed = 0;
    const executor = {
      async execute(request: OperationRequest): Promise<OperationCompletion> {
        executed += 1;
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "succeeded",
          output: { stdout: "" },
        };
      },
    };
    const wf = dynamic(async () => parallel([lint({ id: "lint:1" }), lint({ id: "lint:1" })]));
    await expect(run(wf, { executor })).rejects.toBeInstanceOf(DuplicateCommandIdError);
    expect(executed).toBe(0);
  });

  it("accept an empty dynamic result only through the author's own branch", async () => {
    const none = dynamic(async () => {
      const found: Package[] = [];
      if (found.length === 0) return [];
      return parallel(found.map((pkg) => inspectPackage({ input: pkg })));
    });
    const result = await createHarness().run(none);
    expect(result.output).toEqual([]);
  });

  it("run the same helper for fixed groups inside a workflow", async () => {
    const h = createHarness({ fallback: (r) => `did ${r.commandId}` });
    const wf = dynamic(async () => {
      const [a, b] = await parallel(lint(), format({ id: "format:pre" }));
      await format();
      return [a.stdout, b.stdout];
    });
    const result = await h.run(wf);
    expect(result.output).toEqual(["did lint", "did format:pre"]);
    expect(result.commands.map((c) => c.id)).toEqual(["lint", "format:pre", "format"]);
  });
});

describe("sequences as awaitable data", () => {
  const fixed = sequence(lint(), parallel(format({ id: "format:a" }), format({ id: "format:b" })));

  it("run through the workflow runtime when awaited in a body", async () => {
    const h = createHarness({ fallback: (r) => `did ${r.commandId}` });
    const wf = dynamic(async () => {
      const group = await fixed;
      return group.map((output) => output.stdout);
    });
    const result = await h.run(wf);
    expect(result.output).toEqual(["did format:a", "did format:b"]);
    expect(result.commands.map((c) => c.id)).toEqual(["lint", "format:a", "format:b"]);
  });

  it("refuses to finalize when commands in a sequence are never awaited", async () => {
    const h = createHarness({ fallback: () => "ok" });
    await expect(
      h.run(
        dynamic(async () => {
          sequence(lint(), format());
          return "done";
        }),
      ),
    ).rejects.toThrow("workflow body returned without awaiting 2 operation(s): lint, format");
    expect(h.executed).toHaveLength(0);
  });
});

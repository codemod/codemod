import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { createHarness, failed } from "../src/harness.ts";
import {
  NondeterminismError,
  PROTOCOL_VERSION,
  PlanValidationError,
  TargetValidationError,
  ai,
  canonicalJson,
  exec,
  guard,
  isOperation,
  jssg,
  normalizeTarget,
  parallel,
  plan,
  workflow,
  type JssgRunnable,
  type Target,
} from "../src/index.ts";

const web: Target = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };

const renameApi = jssg({ name: "rename-api", package: "@codemod/rename-api" });
const updateImports = jssg({ name: "update-imports", package: "@codemod/update-imports" });
const format = exec({ name: "format", command: "npm run format" });

const Project = guard(
  "Project",
  (v: unknown): v is { path: string } => typeof v === "object" && v !== null,
);
const migrate = jssg({ name: "migrate", package: "@codemod/migrate", input: Project });

describe("target normalization", () => {
  it("normalizes the root and keeps patterns as written", () => {
    expect(normalizeTarget({ root: "./apps/web/" }, "t")).toEqual({ root: "apps/web" });
    expect(normalizeTarget({ root: "apps//web/./src" }, "t")).toEqual({ root: "apps/web/src" });
    expect(normalizeTarget({ root: "." }, "t")).toEqual({ root: "." });
    expect(normalizeTarget({ root: "apps/.." }, "t")).toEqual({ root: "." });
    expect(normalizeTarget({ root: "packages\\client" }, "t")).toEqual({
      root: "packages/client",
    });
    expect(normalizeTarget(web, "t")).toEqual(web);
    expect(normalizeTarget({ include: ["**/*.ts"] }, "t")).toEqual({ include: ["**/*.ts"] });
  });

  it.each([
    ["not an object", "apps/web", /must be an object/],
    ["an empty target", {}, /at least one of root, include, or exclude/],
    ["an unknown field", { root: "apps", files: ["a"] }, /unknown target field 'files'/],
    ["an empty root", { root: "  " }, /root must be a non-empty relative path/],
    ["an absolute posix root", { root: "/apps/web" }, /must be relative/],
    ["an absolute windows root", { root: "C:\\apps" }, /must be relative/],
    ["a root that escapes", { root: "apps/../../etc" }, /escapes the repository/],
    ["a bare parent root", { root: ".." }, /escapes the repository/],
    ["an empty include list", { include: [] }, /include must be a non-empty list/],
    ["a non-string exclude entry", { exclude: [1] }, /exclude patterns must be non-empty strings/],
    ["an absolute pattern", { include: ["/src/**"] }, /must stay relative to the target root/],
    ["an escaping pattern", { exclude: ["../**"] }, /must stay relative to the target root/],
  ])("rejects %s", (_label, value, message) => {
    expect(() => normalizeTarget(value, "jssg 'x'")).toThrow(TargetValidationError);
    expect(() => normalizeTarget(value, "jssg 'x'")).toThrow(message);
    expect(() => normalizeTarget(value, "jssg 'x'")).toThrow(/^invalid target for jssg 'x': /);
  });
});

describe("JSSG invocation targets", () => {
  it("binds a normalized target and puts it on the wire operation", () => {
    const targeted = renameApi({ target: { root: "./apps/web/", include: ["src/**"] } });
    expect(targeted.kind).toBe("jssg");
    expect(targeted.name).toBe("rename-api");
    expect(targeted.package).toBe("@codemod/rename-api");
    expect(targeted.target).toEqual({ root: "apps/web", include: ["src/**"] });
    expect(targeted.toOperation()).toEqual({
      kind: "jssg",
      package: "@codemod/rename-api",
      target: { root: "apps/web", include: ["src/**"] },
    });
    expect(isOperation(targeted.toOperation())).toBe(true);
  });

  it("matches the shared jssg-target-request fixture", () => {
    const request = {
      protocolVersion: PROTOCOL_VERSION,
      commandId: "rename-api",
      operation: renameApi({ target: web }).toOperation(),
    };
    const fixture = readFileSync(
      join(import.meta.dirname, "..", "fixtures", "protocol", "jssg-target-request.json"),
      "utf8",
    );
    expect(canonicalJson(request)).toBe(canonicalJson(JSON.parse(fixture)));
  });

  it("leaves the definition untargeted and does not mutate it", () => {
    expect(renameApi.target).toBeUndefined();
    renameApi({ target: web });
    expect(renameApi.target).toBeUndefined();
    expect(renameApi.toOperation()).toEqual({ kind: "jssg", package: "@codemod/rename-api" });
  });

  it("keeps the input alongside the target for typed invocations", () => {
    const operation = migrate({ target: { root: "packages/a" } }).toOperation({
      path: "packages/a",
    });
    expect(operation).toEqual({
      kind: "jssg",
      package: "@codemod/migrate",
      target: { root: "packages/a" },
      input: { path: "packages/a" },
    });
  });

  it("rejects invalid targets and anything other than { target } at bind time", () => {
    expect(() => renameApi({ target: { root: "/abs" } })).toThrow(TargetValidationError);
    expect(() => renameApi({ target: {} })).toThrow(/invalid target for jssg 'rename-api'/);
    // @ts-expect-error a target is required
    expect(() => renameApi({})).toThrow(/target must be an object/);
    // @ts-expect-error the prototype call takes only { target }
    expect(() => renameApi({ target: web, id: "x" })).toThrow(
      /unknown invocation field 'id'; pass 'id' to w.run/,
    );
    // @ts-expect-error input still goes to w.run
    expect(() => migrate({ target: web, input: { path: "a" } })).toThrow(
      /unknown invocation field 'input'; pass 'input' to w.run/,
    );
    // @ts-expect-error not an object
    expect(() => renameApi("apps/web")).toThrow(/invocation must be an object/);
  });

  it("does not let a targeted runnable be targeted again", () => {
    const targeted = renameApi({ target: web });
    // @ts-expect-error a targeted runnable is data, not a definition
    expect(() => targeted({ target: web })).toThrow(TypeError);
  });

  it("is not available on exec or ai runnables", () => {
    const summarize = ai({ name: "summarize", prompt: "summarize" });
    // @ts-expect-error exec runnables are not callable
    expect(() => format({ target: web })).toThrow(TypeError);
    // @ts-expect-error ai runnables are not callable
    expect(() => summarize({ target: web })).toThrow(TypeError);
    expect(format.toOperation()).not.toHaveProperty("target");
    expect(summarize.toOperation()).not.toHaveProperty("target");
  });

  it("rejects a 'target' passed through w.run options instead of the definition", async () => {
    const viaOptions = workflow(async (w) => {
      // @ts-expect-error w.run options do not carry a target
      await w.run(renameApi, { target: web });
    });
    const h = createHarness({ fallback: () => "ok" });
    await expect(h.run(viaOptions)).rejects.toThrow(TargetValidationError);
    await expect(h.run(viaOptions)).rejects.toThrow(
      /jssg 'rename-api': w\.run options do not accept 'target'; call the definition instead/,
    );

    const onExec = workflow(async (w) => {
      // @ts-expect-error exec never accepts a target
      await w.run(format, { target: web });
    });
    await expect(h.run(onExec)).rejects.toThrow(
      /exec 'format': w\.run options do not accept 'target'; only JSSG invocations accept a target/,
    );
    expect(h.executed).toHaveLength(0);
  });
});

describe("targeted JSSG in plans and parallel groups", () => {
  it("carries the target into the plan IR, the command record, and the executor request", async () => {
    const fixed = plan(renameApi({ target: web }), updateImports({ target: web }), format);
    expect(fixed.ir.steps).toEqual([
      { type: "run", id: "rename-api", name: "rename-api", kind: "jssg", target: web },
      { type: "run", id: "update-imports", name: "update-imports", kind: "jssg", target: web },
      { type: "run", id: "format", name: "format", kind: "exec" },
    ]);

    const h = createHarness({
      results: { "rename-api": { changed: 3 }, "update-imports": { changed: 1 }, format: "" },
    });
    const result = await h.run(fixed);
    expect(result.output).toEqual([{ changed: 3 }, { changed: 1 }, { stdout: "" }]);
    expect(result.commands.map((c) => c.operation)).toEqual([
      { kind: "jssg", package: "@codemod/rename-api", target: web },
      { kind: "jssg", package: "@codemod/update-imports", target: web },
      { kind: "exec", command: "npm run format" },
    ]);
    expect(h.executed[0]?.operation).toEqual({
      kind: "jssg",
      package: "@codemod/rename-api",
      target: web,
    });

    const replay = await h.reload({ fallback: () => failed("must not execute") }).run(fixed);
    expect(replay.replayed).toBe(true);
    expect(replay.output).toEqual(result.output);
  });

  it("accepts targeted members in a parallel group", async () => {
    const transformA = jssg({ name: "transform-a", package: "@codemod/a" });
    const transformB = jssg({ name: "transform-b", package: "@codemod/b" });
    const group = parallel(transformA({ target: web }), transformB({ target: web }));
    const members: readonly JssgRunnable[] = group.members as readonly JssgRunnable[];
    expect(members.map((m) => m.target)).toEqual([web, web]);

    const h = createHarness({ fallback: () => ({ ok: true }) });
    const result = await h.run(plan(group));
    expect(result.commands.map((c) => c.id)).toEqual(["transform-a", "transform-b"]);
    expect(result.commands.every((c) => c.operation.kind === "jssg" && c.operation.target)).toBe(
      true,
    );
  });

  it("still uses the runnable name as the static id, so two targets of one definition clash", () => {
    expect(() =>
      plan(renameApi({ target: { root: "packages/client" } }), renameApi({ target: web })),
    ).toThrow(PlanValidationError);
  });
});

describe("targets in dynamic workflows and replay", () => {
  const perPackage = (root: string) =>
    workflow(async (w) => {
      const packages = [
        { name: "a", path: "packages/a" },
        { name: "b", path: root },
      ];
      const outputs = [];
      for (const pkg of packages) {
        outputs.push(
          await w.run(migrate({ target: { root: pkg.path } }), {
            input: { path: pkg.path },
            id: `migrate:${pkg.name}`,
          }),
        );
      }
      return outputs;
    });

  it("records the target per explicit id and replays it", async () => {
    const h = createHarness({ fallback: (request) => ({ id: request.commandId }) });
    const first = await h.run(perPackage("packages/b"));
    expect(first.commands.map((c) => [c.id, c.operation])).toEqual([
      [
        "migrate:a",
        {
          kind: "jssg",
          package: "@codemod/migrate",
          target: { root: "packages/a" },
          input: { path: "packages/a" },
        },
      ],
      [
        "migrate:b",
        {
          kind: "jssg",
          package: "@codemod/migrate",
          target: { root: "packages/b" },
          input: { path: "packages/b" },
        },
      ],
    ]);

    const second = await h
      .reload({ fallback: () => failed("must not execute") })
      .run(perPackage("packages/b"));
    expect(second.replayed).toBe(true);
    expect(second.output).toEqual(first.output);
  });

  it("treats a different target under the same id as a changed command", async () => {
    const h = createHarness({ fallback: () => ({}) });
    await h.run(perPackage("packages/b"));

    const replay = h.reload({ fallback: () => failed("must not execute") });
    const error = await replay.run(perPackage("packages/b-renamed")).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(NondeterminismError);
    expect((error as NondeterminismError).kind).toBe("changed");
    expect((error as NondeterminismError).detail).toMatchObject({
      position: 1,
      actualId: "migrate:b",
    });
    expect(replay.executed).toHaveLength(0);
  });

  it("treats adding a target to a previously untargeted command as a change", async () => {
    const h = createHarness({ fallback: () => ({}) });
    await h.run(workflow((w) => w.run(renameApi)));

    const replay = h.reload({ fallback: () => failed("must not execute") });
    const error = await replay
      .run(workflow((w) => w.run(renameApi({ target: web }))))
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(NondeterminismError);
    expect((error as NondeterminismError).kind).toBe("changed");
  });

  it("produces identical commands for equivalent target spellings", async () => {
    const h = createHarness({ fallback: () => ({}) });
    await h.run(workflow((w) => w.run(renameApi({ target: { root: "apps/web" } }))));
    const replay = await h
      .reload({ fallback: () => failed("must not execute") })
      .run(workflow((w) => w.run(renameApi({ target: { root: "./apps/web/" } }))));
    expect(replay.replayed).toBe(true);
  });
});

describe("protocol validation of targets", () => {
  it("accepts well-formed and rejects malformed jssg targets on the wire", () => {
    const base = { kind: "jssg", package: "@codemod/x" };
    expect(isOperation({ ...base, target: { root: "a" } })).toBe(true);
    expect(isOperation({ ...base, target: { include: ["a"], exclude: ["b"] } })).toBe(true);
    expect(isOperation({ ...base, target: {} })).toBe(true);
    expect(isOperation({ ...base, target: "apps/web" })).toBe(false);
    expect(isOperation({ ...base, target: { root: 1 } })).toBe(false);
    expect(isOperation({ ...base, target: { include: "src/**" } })).toBe(false);
    expect(isOperation({ ...base, target: { exclude: [null] } })).toBe(false);
    expect(isOperation({ ...base, target: { root: "a", files: ["a.ts"] } })).toBe(false);
  });

  it("rejects a target on exec and ai operations instead of ignoring it", () => {
    expect(isOperation({ kind: "exec", command: "x" })).toBe(true);
    expect(isOperation({ kind: "exec", command: "x", target: web })).toBe(false);
    expect(isOperation({ kind: "exec", command: "x", target: {} })).toBe(false);
    expect(isOperation({ kind: "ai", prompt: "p" })).toBe(true);
    expect(isOperation({ kind: "ai", prompt: "p", target: web })).toBe(false);
    expect(isOperation({ kind: "ai", prompt: "p", target: {} })).toBe(false);
  });

  it("rejects fields that belong to another operation kind", () => {
    expect(isOperation({ kind: "exec", command: "x", package: "p" })).toBe(false);
    expect(isOperation({ kind: "jssg", package: "p", command: "x" })).toBe(false);
    expect(isOperation({ kind: "ai", prompt: "p", env: {} })).toBe(false);
  });
});

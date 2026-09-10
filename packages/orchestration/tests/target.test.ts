import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { createHarness, failed } from "../src/harness.ts";
import {
  InvocationError,
  NondeterminismError,
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
  type Target,
} from "../src/index.ts";

const web: Target = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };

const renameApi = jssg({ name: "rename-api", script: "rename-api.ts", language: "typescript" });
const updateImports = jssg({
  name: "update-imports",
  script: "update-imports.ts",
  language: "typescript",
});
const format = exec({ name: "format", command: "npm run format" });

const Project = guard(
  "Project",
  (v: unknown): v is { path: string } => typeof v === "object" && v !== null,
);
const migrate = jssg({
  name: "migrate",
  script: "migrate.ts",
  language: "typescript",
  input: Project,
});

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
    // `..` inside a name is not an escape.
    expect(normalizeTarget({ root: "apps/a..b", include: ["foo..bar/**"] }, "t")).toEqual({
      root: "apps/a..b",
      include: ["foo..bar/**"],
    });
  });

  it.each([
    ["not an object", "apps/web", /must be an object/],
    ["an empty target", {}, /at least one of root, include, or exclude/],
    ["an unknown field", { root: "apps", files: ["a"] }, /unknown target field 'files'/],
    ["an empty root", { root: "  " }, /root must be a non-empty relative path/],
    ["an absolute posix root", { root: "/apps/web" }, /must be relative/],
    ["an absolute windows root", { root: "C:\\apps" }, /must be relative/],
    ["a unc root", { root: "\\\\server\\share" }, /must be relative/],
    ["a root that escapes", { root: "apps/../../etc" }, /escapes the repository/],
    ["a root that escapes with backslashes", { root: "apps\\..\\..\\etc" }, /escapes/],
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
  it("binds a normalized target on the command and puts it on the wire operation", () => {
    const targeted = renameApi({ target: { root: "./apps/web/", include: ["src/**"] } });
    expect(targeted.type).toBe("command");
    expect(targeted.id).toBe("rename-api");
    expect(targeted.runnable).toBe(renameApi);
    expect(targeted.target).toEqual({ root: "apps/web", include: ["src/**"] });
    const operation = renameApi.toOperation(undefined, targeted.target);
    expect(operation).toEqual({
      kind: "jssg",
      script: "rename-api.ts",
      language: "typescript",
      target: { root: "apps/web", include: ["src/**"] },
    });
    expect(isOperation(operation)).toBe(true);
  });

  it("matches the shared jssg-target-request fixture", async () => {
    const fixtureRunnable = jssg({
      name: "rename-api",
      script: "scripts/rename-api.ts",
      language: "typescript",
      include: ["**/*.ts"],
      exclude: ["**/*.d.ts"],
    });
    const h = createHarness({ fallback: () => ({}) });
    const result = await h.run(workflow(() => fixtureRunnable({ target: web })));
    const fixture = readFileSync(
      join(import.meta.dirname, "..", "fixtures", "protocol", "jssg-target-request.json"),
      "utf8",
    );
    expect(canonicalJson(h.executed[0])).toBe(canonicalJson(JSON.parse(fixture)));
    expect(result.commands[0]?.operation).toEqual(JSON.parse(fixture).operation);
  });

  it("leaves the definition untargeted and does not mutate it", () => {
    renameApi({ target: web });
    expect(renameApi).not.toHaveProperty("target");
    expect(renameApi.toOperation()).toEqual({
      kind: "jssg",
      script: "rename-api.ts",
      language: "typescript",
    });
    expect(renameApi()).not.toHaveProperty("target");
  });

  it("keeps the input alongside the target for typed invocations", async () => {
    const h = createHarness({ fallback: () => ({}) });
    const result = await h.run(
      workflow(() => migrate({ target: { root: "packages/a" }, input: { path: "packages/a" } })),
    );
    expect(result.commands[0]?.operation).toEqual({
      kind: "jssg",
      script: "migrate.ts",
      language: "typescript",
      target: { root: "packages/a" },
      input: { path: "packages/a" },
    });
    expect(result.commands[0]?.input).toEqual({ path: "packages/a" });
  });

  it("rejects invalid targets and unknown fields when the command is created", () => {
    expect(() => renameApi({ target: { root: "/abs" } })).toThrow(TargetValidationError);
    expect(() => renameApi({ target: {} })).toThrow(/invalid target for jssg 'rename-api'/);
    // @ts-expect-error a target must be an object
    expect(() => renameApi({ target: "apps/web" })).toThrow(/target must be an object/);
    // @ts-expect-error unknown invocation field
    expect(() => renameApi({ target: web, files: ["a"] })).toThrow(InvocationError);
    // @ts-expect-error unknown invocation field
    expect(() => renameApi({ target: web, files: ["a"] })).toThrow(
      /invalid invocation of jssg 'rename-api': unknown invocation field 'files'/,
    );
    // @ts-expect-error not an object
    expect(() => renameApi("apps/web")).toThrow(/invocation options must be an object/);
    // @ts-expect-error id must be a string
    expect(() => renameApi({ id: 3 })).toThrow(/id must be a non-empty string/);
  });

  it("is not available on exec or ai invocations", () => {
    const summarize = ai({ name: "summarize", prompt: "summarize" });
    // @ts-expect-error exec invocations never take a target
    expect(() => format({ target: web })).toThrow(TargetValidationError);
    // @ts-expect-error exec invocations never take a target
    expect(() => format({ target: web })).toThrow(
      /invalid target for exec 'format': exec does not accept a target; only JSSG invocations select files/,
    );
    // @ts-expect-error ai invocations never take a target
    expect(() => summarize({ target: web })).toThrow(TargetValidationError);
    expect(format.toOperation()).not.toHaveProperty("target");
    expect(summarize.toOperation()).not.toHaveProperty("target");
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
      { kind: "jssg", script: "rename-api.ts", language: "typescript", target: web },
      { kind: "jssg", script: "update-imports.ts", language: "typescript", target: web },
      { kind: "exec", command: "npm run format" },
    ]);
    expect(h.executed[0]?.operation).toEqual({
      kind: "jssg",
      script: "rename-api.ts",
      language: "typescript",
      target: web,
    });

    const replay = await h.reload({ fallback: () => failed("must not execute") }).run(fixed);
    expect(replay.replayed).toBe(true);
    expect(replay.output).toEqual(result.output);
  });

  it("accepts targeted members in a parallel group", async () => {
    const transformA = jssg({ name: "transform-a", script: "a.ts", language: "typescript" });
    const transformB = jssg({ name: "transform-b", script: "b.ts", language: "typescript" });
    const group = parallel(transformA({ target: web }), transformB({ target: web }));
    expect(group.members.map((m) => m.target)).toEqual([web, web]);

    const h = createHarness({ fallback: () => ({ ok: true }) });
    const result = await h.run(plan(group));
    expect(result.commands.map((c) => c.id)).toEqual(["transform-a", "transform-b"]);
    expect(result.commands.every((c) => c.operation.kind === "jssg" && c.operation.target)).toBe(
      true,
    );
  });

  it("targets one definition twice in a plan when the invocations carry distinct ids", () => {
    expect(() =>
      plan(renameApi({ target: { root: "packages/client" } }), renameApi({ target: web })),
    ).toThrow(PlanValidationError);
    const fixed = plan(
      renameApi({ target: { root: "packages/client" }, id: "rename-api:client" }),
      renameApi({ target: web, id: "rename-api:web" }),
    );
    expect(fixed.ir.steps.map((s) => (s.type === "run" ? [s.id, s.target] : []))).toEqual([
      ["rename-api:client", { root: "packages/client" }],
      ["rename-api:web", web],
    ]);
  });
});

describe("targets in dynamic workflows and replay", () => {
  const perPackage = (root: string) =>
    workflow(async () => {
      const packages = [
        { name: "a", path: "packages/a" },
        { name: "b", path: root },
      ];
      const outputs = [];
      for (const pkg of packages) {
        outputs.push(
          await migrate({
            input: { path: pkg.path },
            target: { root: pkg.path },
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
          script: "migrate.ts",
          language: "typescript",
          target: { root: "packages/a" },
          input: { path: "packages/a" },
        },
      ],
      [
        "migrate:b",
        {
          kind: "jssg",
          script: "migrate.ts",
          language: "typescript",
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
    await h.run(workflow(() => renameApi()));

    const replay = h.reload({ fallback: () => failed("must not execute") });
    const error = await replay
      .run(workflow(() => renameApi({ target: web })))
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(NondeterminismError);
    expect((error as NondeterminismError).kind).toBe("changed");
  });

  it("produces identical commands for equivalent target spellings", async () => {
    const h = createHarness({ fallback: () => ({}) });
    await h.run(workflow(() => renameApi({ target: { root: "apps/web" } })));
    const replay = await h
      .reload({ fallback: () => failed("must not execute") })
      .run(workflow(() => renameApi({ target: { root: "./apps/web/" } })));
    expect(replay.replayed).toBe(true);
  });
});

describe("protocol validation of targets", () => {
  it("validates intrinsic JSSG applicability and semantic configuration", () => {
    const base = { kind: "jssg", script: "x.ts", language: "typescript" };
    expect(
      isOperation({
        ...base,
        include: ["**/*.ts"],
        exclude: ["**/*.d.ts"],
        semanticAnalysis: { mode: "workspace", root: "src" },
      }),
    ).toBe(true);
    expect(isOperation({ ...base, include: "**/*.ts" })).toBe(false);
    expect(isOperation({ ...base, include: [] })).toBe(false);
    expect(isOperation({ ...base, exclude: [""] })).toBe(false);
    expect(isOperation({ ...base, semanticAnalysis: "repository" })).toBe(false);
    expect(isOperation({ ...base, semanticAnalysis: { mode: "file", root: "src" } })).toBe(false);
    expect(isOperation({ ...base, semanticAnalysis: { mode: "workspace", threads: 4 } })).toBe(
      false,
    );
  });

  it("accepts well-formed and rejects malformed jssg targets on the wire", () => {
    const base = { kind: "jssg", script: "x.ts", language: "typescript" };
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

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { createHarness, failed, type Harness, type HarnessRun } from "./support/harness.ts";
import {
  DuplicateCommandIdError,
  agent,
  canonicalJson,
  shell,
  guard,
  isOperation,
  jssg,
  normalizeTarget,
  parallel,
  sequence,
  dynamic,
  type Target,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

const web: Target = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };

const transform = (name: string) => jssg({ name, transform: ref(name), language: "typescript" });
const renameApi = transform("rename-api");
const updateImports = transform("update-imports");
const format = shell({ name: "format", command: "npm run format" });
const summarize = agent({ name: "summarize", prompt: "summarize" });

const Project = guard(
  "Project",
  (v: unknown): v is { path: string } => typeof v === "object" && v !== null,
);
const migrate = jssg({
  name: "migrate",
  transform: ref("migrate"),
  language: "typescript",
  input: Project,
});

const replaying = (h: Harness) => h.reload({ fallback: () => failed("must not execute") });

/** Matches a thrown error by its class name, its message, and any other fields. */
const error = (name: string, message = /./, fields: object = {}) =>
  expect.objectContaining({ name, message: expect.stringMatching(message), ...fields });
const migrateOperation = (path: string) => ({
  kind: "jssg",
  transform: ref("migrate"),
  language: "typescript",
  target: { root: path },
  input: { path },
});

describe("target normalization", () => {
  it.each<[string, Target, Target]>([
    ["strips a leading ./ and a trailing slash", { root: "./apps/web/" }, { root: "apps/web" }],
    ["collapses // and /./ in the root", { root: "apps//web/./src" }, { root: "apps/web/src" }],
    ["keeps . as the repository root", { root: "." }, { root: "." }],
    ["resolves .. that stays inside the repository", { root: "apps/.." }, { root: "." }],
    ["converts backslashes in the root", { root: "packages\\client" }, { root: "packages/client" }],
    ["keeps include and exclude patterns as written", web, web],
    ["accepts a target without a root", { include: ["**/*.ts"] }, { include: ["**/*.ts"] }],
    [
      "treats .. inside a name as part of the name",
      { root: "apps/a..b", include: ["foo..bar/**"] },
      { root: "apps/a..b", include: ["foo..bar/**"] },
    ],
  ])("%s", (_label, target, expected) => {
    expect(normalizeTarget(target, "t")).toEqual(expected);
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
    expect(() => normalizeTarget(value, "jssg 'x'")).toThrow(
      error("TargetValidationError", message),
    );
  });

  it("names the invocation before the reason", () => {
    expect(() => normalizeTarget({}, "jssg 'x'")).toThrow(/^invalid target for jssg 'x': /);
  });
});

describe("JSSG invocation targets", () => {
  it("binds the normalized target to the command", () => {
    const { type, id, runnable, target } = renameApi({
      target: { root: "./apps/web/", include: ["src/**"] },
    });
    expect({ type, id, runnable, target }).toEqual({
      type: "command",
      id: "rename-api",
      runnable: renameApi,
      target: { root: "apps/web", include: ["src/**"] },
    });
  });

  it("puts the target on the wire operation", () => {
    expect(renameApi.toOperation(undefined, { root: "apps/web" })).toEqual({
      kind: "jssg",
      transform: ref("rename-api"),
      language: "typescript",
      target: { root: "apps/web" },
    });
  });

  it("sends the request in the shared jssg-target-request fixture", async () => {
    const fixtureRunnable = jssg({
      name: "rename-api",
      transform: {
        name: "rename-api",
        hash: "9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c5b4a39281706f5e4d3c2b1a0",
      },
      language: "typescript",
      include: ["**/*.ts"],
      exclude: ["**/*.d.ts"],
    });
    const h = createHarness({ fallback: () => ({}) });
    await h.run(dynamic(() => fixtureRunnable({ target: web })));
    const fixture = readFileSync(
      join(import.meta.dirname, "..", "fixtures", "protocol", "jssg-target-request.json"),
      "utf8",
    );
    expect(canonicalJson(h.executed[0])).toBe(canonicalJson(JSON.parse(fixture)));
  });

  it("leaves the definition and its later invocations untargeted", () => {
    renameApi({ target: web });
    expect([
      "target" in renameApi,
      "target" in renameApi.toOperation(),
      "target" in renameApi(),
    ]).toEqual([false, false, false]);
  });

  it("keeps the input alongside the target on typed invocations", async () => {
    const result = await createHarness({ fallback: () => ({}) }).run(
      dynamic(() => migrate({ target: { root: "packages/a" }, input: { path: "packages/a" } })),
    );
    expect(result.commands[0]).toMatchObject({
      operation: migrateOperation("packages/a"),
      input: { path: "packages/a" },
    });
  });

  it.each<[string, () => unknown, unknown]>([
    [
      "an invalid target",
      () => renameApi({ target: { root: "/abs" } }),
      error("TargetValidationError", /^invalid target for jssg 'rename-api': .*must be relative/),
    ],
    [
      "an empty target",
      () => renameApi({ target: {} }),
      error("TargetValidationError", /^invalid target for jssg 'rename-api'/),
    ],
    [
      "a target that is not an object",
      // @ts-expect-error a target must be an object
      () => renameApi({ target: "apps/web" }),
      error("TargetValidationError", /target must be an object/),
    ],
    [
      "an unknown invocation field",
      // @ts-expect-error unknown invocation field
      () => renameApi({ target: web, files: ["a"] }),
      error(
        "InvocationError",
        /^invalid invocation of jssg 'rename-api': unknown invocation field 'files'/,
      ),
    ],
    [
      "options that are not an object",
      // @ts-expect-error not an object
      () => renameApi("apps/web"),
      error("InvocationError", /invocation options must be an object/),
    ],
    [
      "an id that is not a string",
      // @ts-expect-error id must be a string
      () => renameApi({ id: 3 }),
      error("InvocationError", /id must be a non-empty string/),
    ],
    [
      "a target on a shell invocation",
      // @ts-expect-error shell invocations never take a target
      () => format({ target: web }),
      error(
        "TargetValidationError",
        /^invalid target for shell 'format': shell does not accept a target; only JSSG invocations select files/,
      ),
    ],
    [
      "a target on an agent invocation",
      // @ts-expect-error agent invocations never take a target
      () => summarize({ target: web }),
      error("TargetValidationError"),
    ],
  ])("rejects %s when the command is created", (_label, invoke, expected) => {
    expect(invoke).toThrow(expected as Error);
  });
});

describe("targeted JSSG in static composition", () => {
  const fixed = sequence(renameApi({ target: web }), updateImports({ target: web }), format());
  const h = createHarness({
    results: { "rename-api": { changed: 3 }, "update-imports": { changed: 1 }, format: "" },
  });
  beforeAll(async () => {
    await h.run(fixed);
  });

  it("carries each stage's target into the sequence IR", () => {
    expect(
      fixed.ir.root.stages.map((stage) => (stage.type === "operation" ? stage.target : null)),
    ).toEqual([web, web, undefined]);
  });

  it("sends each stage's target in its executor request", () => {
    expect(h.executed.map((request) => request.operation)).toEqual([
      { kind: "jssg", transform: ref("rename-api"), language: "typescript", target: web },
      { kind: "jssg", transform: ref("update-imports"), language: "typescript", target: web },
      { kind: "shell", command: "npm run format" },
    ]);
  });

  it("replays a targeted sequence from history without executing", async () => {
    const replay = await replaying(h).run(fixed);
    expect([replay.replayed, replay.output]).toEqual([true, { stdout: "" }]);
  });

  const group = parallel(
    transform("transform-a")({ target: web }),
    transform("transform-b")({ target: web }),
  );

  it("carries each parallel member's target into the IR", () => {
    expect(
      group.ir.root.members.map((member) =>
        member.type === "operation" ? member.target : undefined,
      ),
    ).toEqual([web, web]);
  });

  it("sends each parallel member its target", async () => {
    const result = await createHarness({ fallback: () => ({ ok: true }) }).run(group);
    expect(
      result.commands.map((c) => [c.id, c.operation.kind === "jssg" && c.operation.target]),
    ).toEqual([
      ["transform-a", web],
      ["transform-b", web],
    ]);
  });

  it("rejects targeting one definition twice without distinct ids", () => {
    expect(() =>
      sequence(renameApi({ target: { root: "packages/client" } }), renameApi({ target: web })),
    ).toThrow(DuplicateCommandIdError);
  });

  it("targets one definition twice under distinct ids", () => {
    const twice = sequence(
      renameApi({ target: { root: "packages/client" }, id: "rename-api:client" }),
      renameApi({ target: web, id: "rename-api:web" }),
    );
    expect(
      twice.ir.root.stages.map((stage) =>
        stage.type === "operation" ? [stage.id, stage.target] : [],
      ),
    ).toEqual([
      ["rename-api:client", { root: "packages/client" }],
      ["rename-api:web", web],
    ]);
  });
});

describe("targets in dynamic workflows and replay", () => {
  const perPackage = (b: string) =>
    dynamic(async () => {
      const outputs = [];
      for (const [name, path] of [
        ["a", "packages/a"],
        ["b", b],
      ] as const) {
        outputs.push(
          await migrate({ input: { path }, target: { root: path }, id: `migrate:${name}` }),
        );
      }
      return outputs;
    });

  const h = createHarness({ fallback: (request) => ({ id: request.commandId }) });
  let first: HarnessRun<unknown>;
  beforeAll(async () => {
    first = await h.run(perPackage("packages/b"));
  });

  it("records each invocation's target and input under its explicit id", () => {
    expect(first.commands.map((c) => [c.id, c.operation])).toEqual([
      ["migrate:a", migrateOperation("packages/a")],
      ["migrate:b", migrateOperation("packages/b")],
    ]);
  });

  it("replays recorded targets without executing", async () => {
    const second = await replaying(h).run(perPackage("packages/b"));
    expect([second.replayed, second.output]).toEqual([true, first.output]);
  });

  it("fails replay as changed, executing nothing, when an id gets a different target", async () => {
    const replay = replaying(h);
    const thrown = await replay.run(perPackage("packages/b-renamed")).catch((e: unknown) => e);
    expect([thrown, replay.executed]).toEqual([
      error("NondeterminismError", /./, {
        kind: "changed",
        detail: expect.objectContaining({ position: 1, actualId: "migrate:b" }),
      }),
      [],
    ]);
  });

  it("fails replay as changed when a target is added to an untargeted command", async () => {
    const untargeted = createHarness({ fallback: () => ({}) });
    await untargeted.run(dynamic(() => renameApi()));
    await expect(
      replaying(untargeted).run(dynamic(() => renameApi({ target: web }))),
    ).rejects.toThrow(error("NondeterminismError", /./, { kind: "changed" }));
  });

  it("replays equivalent target spellings as the same command", async () => {
    const spelled = createHarness({ fallback: () => ({}) });
    await spelled.run(dynamic(() => renameApi({ target: { root: "apps/web" } })));
    const replay = await replaying(spelled).run(
      dynamic(() => renameApi({ target: { root: "./apps/web/" } })),
    );
    expect(replay.replayed).toBe(true);
  });
});

describe("protocol validation of operations", () => {
  const jssgOp = { kind: "jssg", transform: ref("x"), language: "typescript" };
  const shellOp = { kind: "shell", command: "x" };
  const agentOp = { kind: "agent", prompt: "p", backend: { kind: "builtin", tools: [] } };
  const assessmentOp = {
    kind: "assessment",
    state: "s",
    questions: { ok: { type: "yesno", instructions: "ok?" } },
    provider: "systemone",
    model: "jev-latest",
  };

  it.each<[string, object]>([
    [
      "jssg include, exclude, and workspace semantic analysis",
      {
        ...jssgOp,
        include: ["**/*.ts"],
        exclude: ["**/*.d.ts"],
        semanticAnalysis: { mode: "workspace", root: "src" },
      },
    ],
    ["a jssg target with a root", { ...jssgOp, target: { root: "a" } }],
    [
      "a jssg target with include and exclude",
      { ...jssgOp, target: { include: ["a"], exclude: ["b"] } },
    ],
    ["an empty jssg target", { ...jssgOp, target: {} }],
    ["a shell operation", shellOp],
    ["an agent operation", agentOp],
    ["an assessment operation", assessmentOp],
  ])("accepts %s", (_label, operation) => {
    expect(isOperation(operation)).toBe(true);
  });

  it.each<[string, object]>([
    ["a jssg include that is not a list", { ...jssgOp, include: "**/*.ts" }],
    ["an empty jssg include list", { ...jssgOp, include: [] }],
    ["a blank jssg exclude pattern", { ...jssgOp, exclude: [""] }],
    ["semantic analysis given as a string", { ...jssgOp, semanticAnalysis: "repository" }],
    [
      "file-mode semantic analysis with a root",
      { ...jssgOp, semanticAnalysis: { mode: "file", root: "src" } },
    ],
    [
      "an unknown semantic analysis field",
      { ...jssgOp, semanticAnalysis: { mode: "workspace", threads: 4 } },
    ],
    ["a jssg target that is a string", { ...jssgOp, target: "apps/web" }],
    ["a jssg target root that is not a string", { ...jssgOp, target: { root: 1 } }],
    ["a jssg target include that is not a list", { ...jssgOp, target: { include: "src/**" } }],
    ["a null jssg target exclude pattern", { ...jssgOp, target: { exclude: [null] } }],
    ["an unknown jssg target field", { ...jssgOp, target: { root: "a", files: ["a.ts"] } }],
    ["a package field on a shell operation", { ...shellOp, package: "p" }],
    [
      "package and command fields on a jssg operation",
      { kind: "jssg", package: "p", command: "x" },
    ],
    ["an env field on an agent operation", { ...agentOp, env: {} }],
    ["an unknown operation kind", { kind: "ai", prompt: "p" }],
  ])("rejects %s", (_label, operation) => {
    expect(isOperation(operation)).toBe(false);
  });

  it.each<[string, object]>([
    ["a shell", shellOp],
    ["an agent", agentOp],
    ["an assessment", assessmentOp],
  ])("rejects a target, even an empty one, on %s operation", (_kind, operation) => {
    expect([web, {}].some((target) => isOperation({ ...operation, target }))).toBe(false);
  });
});

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  PROTOCOL_VERSION,
  shell,
  isFileOutcomes,
  isOperation,
  isOperationCompletion,
  isOperationRequest,
  isSelector,
  parseCompletion,
} from "../src/index.ts";
import { ref } from "./helpers.ts";

const dir = join(import.meta.dirname, "..", "fixtures", "protocol");
const text = (name: string) => readFileSync(join(dir, name), "utf8");
const fixture = (name: string) => JSON.parse(text(name)) as unknown;
const fixtures = (suffix: string) => readdirSync(dir).filter((name) => name.endsWith(suffix));

describe("protocol fixtures shared with crates/orchestration-agent", () => {
  it.each(fixtures("-request.json"))("accepts %s at the current protocol version", (name) => {
    const value = fixture(name) as { protocolVersion: number };
    expect([isOperationRequest(value), value.protocolVersion]).toEqual([true, PROTOCOL_VERSION]);
  });

  it.each(fixtures("-completion.json"))("accepts and round-trips %s", (name) => {
    expect([isOperationCompletion(fixture(name)), parseCompletion(text(name))]).toEqual([
      true,
      fixture(name),
    ]);
  });

  it("serializes a shell runnable as the shell request fixture", () => {
    const inspect = shell({
      name: "inspect",
      command: `printf '{"needsMigration":true}'`,
      env: { CI: "1" },
    });
    const request = {
      protocolVersion: PROTOCOL_VERSION,
      commandId: "inspect",
      operation: inspect.toOperation(),
    };
    expect(request).toEqual(fixture("shell-request.json"));
  });
});

describe("operation requests", () => {
  const base = fixture("jssg-request.json") as Record<string, unknown>;
  const withContext = (context: unknown) => ({ ...base, context });

  it.each<[string, unknown]>([
    ["a target root context", withContext({ targetRoot: "/r" })],
    ["an empty context", withContext({})],
    [
      "a batch of safe relative files and the artifact source",
      withContext({ files: [{ path: "src/a.ts", content: "" }], artifact: { source: "" } }),
    ],
  ])("accepts a request with %s", (_name, value) => {
    expect(isOperationRequest(value)).toBe(true);
  });

  it.each<[string, unknown]>([
    ["a blank target root", withContext({ targetRoot: " " })],
    ["the removed script root", withContext({ scriptRoot: "/w" })],
    ["an artifact with extra fields", withContext({ artifact: { source: "", hash: "h" } })],
    ["an artifact without source", withContext({ artifact: {} })],
    ["an unknown context field", withContext({ cwd: "/tmp" })],
    ["a non-object context", withContext("/tmp")],
    ["a context field on the envelope", { ...base, targetRoot: "/tmp" }],
    ["an absolute batch path", withContext({ files: [{ path: "/a.ts", content: "" }] })],
    [
      "an unknown batch file field",
      withContext({ files: [{ path: "a.ts", content: "", mode: 1 }] }),
    ],
  ])("rejects a request with %s", (_name, value) => {
    expect(isOperationRequest(value)).toBe(false);
  });
});

describe("operations", () => {
  const jssg = { kind: "jssg", transform: ref("x"), language: "typescript" };
  const hash = "a".repeat(64);

  it.each<[string, unknown]>([
    ["a jssg operation with a transform reference", jssg],
    ["a target root with '..' inside a name", { ...jssg, target: { root: "apps/a..b" } }],
    ["file-mode semantic analysis", { ...jssg, semanticAnalysis: { mode: "file" } }],
  ])("accepts %s", (_name, value) => {
    expect(isOperation(value)).toBe(true);
  });

  it.each<[string, unknown]>([
    ["no transform", { ...jssg, transform: undefined }],
    ["a transform path instead of a reference", { ...jssg, transform: "scripts/x.ts" }],
    ["a transform without a hash", { ...jssg, transform: { name: "x" } }],
    ["a transform with an empty name", { ...jssg, transform: { name: "", hash } }],
    ["a transform hash of 63 digits", { ...jssg, transform: { name: "x", hash: "a".repeat(63) } }],
    ["an uppercase transform hash", { ...jssg, transform: { name: "x", hash: "A".repeat(64) } }],
    ["a transform carrying its source", { ...jssg, transform: { name: "x", hash, source: "" } }],
    ["the removed script field", { ...jssg, script: "scripts/x.ts" }],
    [
      "a root on file-mode semantic analysis",
      { ...jssg, semanticAnalysis: { mode: "file", root: "src" } },
    ],
    ["the removed exec kind", { kind: "exec", command: "true" }],
    ["a target on a shell operation", { kind: "shell", command: "true", target: { root: "a" } }],
  ])("rejects an operation with %s", (_name, value) => {
    expect(isOperation(value)).toBe(false);
  });

  it.each(["/abs/x.ts", "\\\\server\\x.ts", "C:\\x.ts", "c:/x.ts", "../x.ts", " "])(
    "rejects %j as a target root or a workspace analysis root",
    (root) => {
      expect([
        isOperation({ ...jssg, target: { root } }),
        isOperation({ ...jssg, semanticAnalysis: { mode: "workspace", root } }),
      ]).toEqual([false, false]);
    },
  );

  it.each<[string, unknown]>([
    ["a rule", { rule: { pattern: "a($B)" } }],
    [
      "a rule with constraints and utils",
      {
        rule: { matches: "u" },
        constraints: { B: { kind: "string" } },
        utils: { u: { kind: "call" } },
      },
    ],
  ])("accepts a selector with %s, alone and on a jssg operation", (_name, selector) => {
    expect([isSelector(selector), isOperation({ ...jssg, selector })]).toEqual([true, true]);
  });

  it.each<[string, unknown]>([
    ["no rule", { constraints: {} }],
    ["an empty rule", { rule: {} }],
    ["a string rule", { rule: "a($B)" }],
    ["an id", { rule: { pattern: "a" }, id: "s" }],
    ["a language", { rule: { pattern: "a" }, language: "tsx" }],
    ["a fix", { rule: { pattern: "a" }, fix: "b" }],
    ["non-JSON data", { rule: { pattern: "a", when: () => true } }],
    ["a non-object", "a($B)"],
  ])("rejects a selector with %s, alone and on a jssg operation", (_name, selector) => {
    expect([isSelector(selector), isOperation({ ...jssg, selector })]).toEqual([false, false]);
  });
});

describe("batch file outcomes", () => {
  const outcome = (edits: unknown[], extra = {}) => [{ path: "src/a.ts", edits, ...extra }];
  const edit = (fields: object) => outcome([{ path: "src/a.ts", content: "x", ...fields }]);

  it.each<[string, unknown]>([
    ["an edit that renames within the repository", edit({ renameTo: "src/b.ts" })],
    ["a file output and no edits", outcome([], { output: { file: "a" } })],
  ])("accepts %s", (_name, value) => {
    expect(isFileOutcomes(value)).toBe(true);
  });

  it.each<[string, unknown]>([
    ["a rename out of the repository", edit({ renameTo: "../x" })],
    ["an absolute edit path", edit({ path: "/etc/x" })],
    ["an unknown edit field", edit({ kind: "modified" })],
    ["an unknown file field", outcome([], { secondary: [] })],
    ["a parent-relative file path", [{ path: "../a.ts", edits: [] }]],
    ["an object instead of a list", { files: [] }],
  ])("rejects %s", (_name, value) => {
    expect(isFileOutcomes(value)).toBe(false);
  });
});

describe("operation completions", () => {
  const completion = (fields: object) => ({
    protocolVersion: PROTOCOL_VERSION,
    commandId: "x",
    ...fields,
  });
  const success = (fields = {}) => completion({ status: "succeeded", output: null, ...fields });
  const failure = (error: object, extra = {}) => completion({ status: "failed", error, ...extra });

  it("accepts a failure with structured error details", () => {
    const value = failure({ message: "m", details: { phase: "stage" } });
    expect(isOperationCompletion(value)).toBe(true);
  });

  it.each<[string, unknown]>([
    ["an unknown protocol version", success({ protocolVersion: 99 })],
    ["an unknown status", completion({ status: "done" })],
    ["an unknown error field", failure({ message: "m", stack: "s" })],
    ["an unknown envelope field", failure({ message: "m" }, { extra: 1 })],
    ["a succeeded status but no output", completion({ status: "succeeded" })],
    ["a succeeded status and an error", success({ error: { message: "m" } })],
    ["a failed status but no error", completion({ status: "failed" })],
    ["a non-integer exit code", failure({ message: "m", exitCode: "3" })],
  ])("rejects a completion with %s", (_name, value) => {
    expect(isOperationCompletion(value)).toBe(false);
  });

  it.each([
    ["text that is not JSON", "not json", /invalid JSON/],
    ["JSON that is not a completion", '{"protocolVersion":1}', /invalid completion/],
  ])("throws when parsing %s", (_name, value, message) => {
    expect(() => parseCompletion(value)).toThrow(message);
  });
});

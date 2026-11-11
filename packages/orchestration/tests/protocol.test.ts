import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  PROTOCOL_VERSION,
  shell,
  isFileOutcomes,
  isOperationCompletion,
  isSelector,
  parseCompletion,
} from "../src/index.ts";

const dir = join(import.meta.dirname, "..", "fixtures", "protocol");
const text = (name: string) => readFileSync(join(dir, name), "utf8");
const fixture = (name: string) => JSON.parse(text(name)) as unknown;
const fixtures = (suffix: string) => readdirSync(dir).filter((name) => name.endsWith(suffix));

describe("protocol fixtures shared with crates/orchestration-agent", () => {
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

describe("selectors", () => {
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
  ])("accepts a selector with %s", (_name, selector) => {
    expect(isSelector(selector)).toBe(true);
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
  ])("rejects a selector with %s", (_name, selector) => {
    expect(isSelector(selector)).toBe(false);
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

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  PROTOCOL_VERSION,
  canonicalJson,
  exec,
  isFileOutcomes,
  isOperation,
  isOperationCompletion,
  isOperationRequest,
  parseCompletion,
} from "../src/index.ts";

const dir = join(import.meta.dirname, "..", "fixtures", "protocol");
const fixture = (name: string) => JSON.parse(readFileSync(join(dir, name), "utf8")) as unknown;

describe("protocol fixtures shared with crates/execution-bridge", () => {
  it("every fixture is valid and round-trips", () => {
    for (const name of readdirSync(dir).filter((f) => f.endsWith("-request.json"))) {
      const value = fixture(name);
      expect(isOperationRequest(value), name).toBe(true);
      expect((value as { protocolVersion: number }).protocolVersion).toBe(PROTOCOL_VERSION);
    }
    for (const name of readdirSync(dir).filter((f) => f.endsWith("-completion.json"))) {
      const text = readFileSync(join(dir, name), "utf8");
      expect(isOperationCompletion(JSON.parse(text)), name).toBe(true);
      expect(canonicalJson(parseCompletion(text))).toBe(canonicalJson(JSON.parse(text)));
    }
  });

  it("exec runnables produce the fixture request shape", () => {
    const inspect = exec({
      name: "inspect",
      command: `printf '{"needsMigration":true}'`,
      env: { CI: "1" },
    });
    const request = {
      protocolVersion: PROTOCOL_VERSION,
      commandId: "inspect",
      operation: inspect.toOperation(),
    };
    expect(canonicalJson(request)).toBe(canonicalJson(fixture("exec-request.json")));
  });
});

describe("strict validation", () => {
  const base = fixture("jssg-request.json") as Record<string, unknown>;
  const jssg = { kind: "jssg", script: "scripts/x.ts", language: "typescript" };

  it.each<[string, unknown, boolean]>([
    [
      "a strict request context",
      { ...base, context: { scriptRoot: "/w", targetRoot: "/r" } },
      true,
    ],
    ["an empty context", { ...base, context: {} }, true],
    [
      "a batch of safe relative files",
      { ...base, context: { files: [{ path: "src/a.ts", content: "" }] } },
      true,
    ],
    ["a blank script root", { ...base, context: { scriptRoot: " " } }, false],
    ["an unknown context field", { ...base, context: { cwd: "/tmp" } }, false],
    ["a non-object context", { ...base, context: "/tmp" }, false],
    ["a context field on the envelope", { ...base, scriptRoot: "/tmp" }, false],
    [
      "an absolute batch path",
      { ...base, context: { files: [{ path: "/a.ts", content: "" }] } },
      false,
    ],
    [
      "an unknown batch file field",
      { ...base, context: { files: [{ path: "a.ts", content: "", mode: 1 }] } },
      false,
    ],
  ])("request with %s", (_name, value, expected) => {
    expect(isOperationRequest(value)).toBe(expected);
  });

  it("requires jssg script and roots to be safe relative paths on the wire", () => {
    expect(isOperation(jssg)).toBe(true);
    expect(isOperation({ ...jssg, script: "scripts/foo..bar.ts" })).toBe(true);
    for (const bad of ["/abs/x.ts", "\\\\server\\x.ts", "C:\\x.ts", "c:/x.ts", "../x.ts", " "]) {
      expect(isOperation({ ...jssg, script: bad }), bad).toBe(false);
      expect(isOperation({ ...jssg, target: { root: bad } }), bad).toBe(false);
      expect(
        isOperation({ ...jssg, semanticAnalysis: { mode: "workspace", root: bad } }),
        bad,
      ).toBe(false);
    }
    expect(isOperation({ ...jssg, target: { root: "apps/a..b" } })).toBe(true);
    expect(isOperation({ ...jssg, semanticAnalysis: { mode: "file" } })).toBe(true);
    expect(isOperation({ ...jssg, semanticAnalysis: { mode: "file", root: "src" } })).toBe(false);
    expect(isOperation({ kind: "exec", command: "true", target: { root: "a" } })).toBe(false);
  });

  it("validates batch outcomes including every returned path", () => {
    const outcome = (edits: unknown[], extra = {}) => [{ path: "src/a.ts", edits, ...extra }];
    expect(
      isFileOutcomes(outcome([{ path: "src/a.ts", content: "x", renameTo: "src/b.ts" }])),
    ).toBe(true);
    expect(isFileOutcomes(outcome([], { output: { file: "a" } }))).toBe(true);
    expect(isFileOutcomes(outcome([{ path: "src/a.ts", content: "x", renameTo: "../x" }]))).toBe(
      false,
    );
    expect(isFileOutcomes(outcome([{ path: "/etc/x", content: "x" }]))).toBe(false);
    expect(isFileOutcomes(outcome([{ path: "src/a.ts", content: "x", kind: "modified" }]))).toBe(
      false,
    );
    expect(isFileOutcomes(outcome([], { secondary: [] }))).toBe(false);
    expect(isFileOutcomes([{ path: "../a.ts", edits: [] }])).toBe(false);
    expect(isFileOutcomes({ files: [] })).toBe(false);
  });

  it.each<[string, unknown, boolean]>([
    [
      "an unknown protocol version",
      { protocolVersion: 99, commandId: "x", status: "succeeded", output: null },
      false,
    ],
    [
      "an unknown status",
      { protocolVersion: PROTOCOL_VERSION, commandId: "x", status: "done" },
      false,
    ],
    [
      "structured error details",
      {
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "failed",
        error: { message: "m", details: { phase: "stage" } },
      },
      true,
    ],
    [
      "an unknown error field",
      {
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "failed",
        error: { message: "m", stack: "s" },
      },
      false,
    ],
    [
      "an unknown envelope field",
      {
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "failed",
        error: { message: "m" },
        extra: 1,
      },
      false,
    ],
    [
      "success without output",
      { protocolVersion: PROTOCOL_VERSION, commandId: "x", status: "succeeded" },
      false,
    ],
    [
      "success with an error",
      {
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "succeeded",
        output: null,
        error: { message: "m" },
      },
      false,
    ],
    [
      "failure without an error",
      { protocolVersion: PROTOCOL_VERSION, commandId: "x", status: "failed" },
      false,
    ],
    [
      "a non-integer exit code",
      {
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "failed",
        error: { message: "m", exitCode: "3" },
      },
      false,
    ],
  ])("completion with %s", (_name, value, expected) => {
    expect(isOperationCompletion(value)).toBe(expected);
  });

  it("reports invalid completion text", () => {
    expect(() => parseCompletion("not json")).toThrow(/invalid JSON/);
    expect(() => parseCompletion('{"protocolVersion":1}')).toThrow(/invalid completion/);
  });
});

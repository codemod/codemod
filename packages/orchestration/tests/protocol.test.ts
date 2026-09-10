import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  PROTOCOL_VERSION,
  canonicalJson,
  exec,
  isOperation,
  isOperationCompletion,
  isOperationRequest,
  parseCompletion,
} from "../src/index.ts";

const dir = join(import.meta.dirname, "..", "fixtures", "protocol");
const fixture = (name: string) => JSON.parse(readFileSync(join(dir, name), "utf8")) as unknown;

describe("protocol fixtures shared with crates/execution-bridge", () => {
  it("every request fixture is a valid OperationRequest", () => {
    for (const name of readdirSync(dir).filter((f) => f.endsWith("-request.json"))) {
      const value = fixture(name);
      expect(isOperationRequest(value), name).toBe(true);
      expect((value as { protocolVersion: number }).protocolVersion).toBe(PROTOCOL_VERSION);
    }
  });

  it("every completion fixture is a valid OperationCompletion", () => {
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

  it("accepts an optional strict request context that never enters the operation", () => {
    const base = fixture("jssg-request.json") as Record<string, unknown>;
    expect(isOperationRequest({ ...base, context: { scriptRoot: "/tmp/workflow" } })).toBe(true);
    expect(isOperationRequest({ ...base, context: {} })).toBe(true);
    expect(isOperationRequest({ ...base, context: { scriptRoot: " " } })).toBe(false);
    expect(isOperationRequest({ ...base, context: { cwd: "/tmp" } })).toBe(false);
    expect(isOperationRequest({ ...base, context: "/tmp" })).toBe(false);
    expect(isOperationRequest({ ...base, scriptRoot: "/tmp" })).toBe(false);
    const operation = base.operation as Record<string, unknown>;
    expect(isOperationRequest({ ...base, operation: { ...operation, scriptRoot: "/tmp" } })).toBe(
      false,
    );
  });

  it("requires jssg script and roots to be safe relative paths on the wire", () => {
    const base = { kind: "jssg", script: "scripts/x.ts", language: "typescript" };
    expect(isOperation(base)).toBe(true);
    expect(isOperation({ ...base, script: "scripts/foo..bar.ts" })).toBe(true);
    for (const bad of ["/abs/x.ts", "\\\\server\\x.ts", "C:\\x.ts", "c:/x.ts", "../x.ts", " "]) {
      expect(isOperation({ ...base, script: bad }), bad).toBe(false);
      expect(isOperation({ ...base, target: { root: bad } }), bad).toBe(false);
      expect(
        isOperation({ ...base, semanticAnalysis: { mode: "workspace", root: bad } }),
        bad,
      ).toBe(false);
    }
    expect(isOperation({ ...base, target: { root: "apps/a..b" } })).toBe(true);
    expect(
      isOperation({ ...base, semanticAnalysis: { mode: "workspace", root: "src..gen" } }),
    ).toBe(true);
    expect(isOperation({ ...base, semanticAnalysis: { mode: "file" } })).toBe(true);
  });

  it("rejects unknown protocol versions and statuses", () => {
    expect(
      isOperationCompletion({ protocolVersion: 99, commandId: "x", status: "succeeded" }),
    ).toBe(false);
    expect(
      isOperationCompletion({ protocolVersion: PROTOCOL_VERSION, commandId: "x", status: "done" }),
    ).toBe(false);
    expect(() => parseCompletion("not json")).toThrow(/invalid JSON/);
  });

  it("rejects malformed status-dependent completion fields", () => {
    expect(
      isOperationCompletion({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "succeeded",
      }),
    ).toBe(false);
    expect(
      isOperationCompletion({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "succeeded",
        output: null,
        error: { message: "unexpected" },
      }),
    ).toBe(false);
    expect(
      isOperationCompletion({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "failed",
      }),
    ).toBe(false);
    expect(
      isOperationCompletion({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "x",
        status: "failed",
        error: { message: "bad", exitCode: "3" },
      }),
    ).toBe(false);
  });
});

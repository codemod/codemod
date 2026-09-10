import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  PROTOCOL_VERSION,
  canonicalJson,
  exec,
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

  it("rejects unknown protocol versions and statuses", () => {
    expect(isOperationCompletion({ protocolVersion: 2, commandId: "x", status: "succeeded" })).toBe(
      false,
    );
    expect(isOperationCompletion({ protocolVersion: 1, commandId: "x", status: "done" })).toBe(
      false,
    );
    expect(() => parseCompletion("not json")).toThrow(/invalid JSON/);
  });

  it("rejects malformed status-dependent completion fields", () => {
    expect(isOperationCompletion({ protocolVersion: 1, commandId: "x", status: "succeeded" })).toBe(
      false,
    );
    expect(
      isOperationCompletion({
        protocolVersion: 1,
        commandId: "x",
        status: "succeeded",
        output: null,
        error: { message: "unexpected" },
      }),
    ).toBe(false);
    expect(isOperationCompletion({ protocolVersion: 1, commandId: "x", status: "failed" })).toBe(
      false,
    );
    expect(
      isOperationCompletion({
        protocolVersion: 1,
        commandId: "x",
        status: "failed",
        error: { message: "bad", exitCode: "3" },
      }),
    ).toBe(false);
  });
});

/**
 * The portable execution path over the `Host` seam:
 *
 * - `executeJssgCommand` against an in-memory host (no Node filesystem):
 *   selection with ignore files, the batch handed to `dispatch`, containment,
 *   conflict checks, the transactional commit, and nothing written on failure;
 * - `executeShell` against the Node host, with the bridge's DirectRunner
 *   semantics (merged output, line normalization, exit codes, env, shebang
 *   scripts, abort);
 * - the Node host's own contract (EILSEQ, symlink-free directory entries).
 */
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  NODE_HOST,
  PROTOCOL_VERSION,
  executeJssgCommand,
  executeShell,
  normalizeShellOutput,
  type FileOutcome,
  type JssgDispatch,
  type JssgOperation,
  type Json,
  type OperationRequest,
} from "../src/index.ts";
import { artifact } from "./helpers.ts";
import { createMemoryHost, type MemoryHost } from "./fixtures/memory-host.ts";

const built = artifact("transform", "export default () => null;\n");
const operation: JssgOperation = {
  kind: "jssg",
  transform: { name: built.name, hash: built.hash },
  language: "typescript",
};

/** A dispatcher that uppercases every file it is given. */
const upper = (
  seen: OperationRequest[] = [],
  extra: (path: string) => FileOutcome["edits"] = () => [],
) =>
  (async (request) => {
    seen.push(request);
    const files = request.context!.files!.map((file) => ({
      path: file.path,
      edits: [{ path: file.path, content: file.content.toUpperCase() }, ...extra(file.path)],
      output: { file: file.path },
    }));
    return {
      protocolVersion: PROTOCOL_VERSION,
      commandId: request.commandId,
      status: "succeeded",
      output: { files } as unknown as Json,
    } as const;
  }) satisfies JssgDispatch;

describe("executeJssgCommand over an in-memory host", () => {
  let host: MemoryHost;
  beforeEach(() => {
    host = createMemoryHost("/repo");
    host.write("/repo/a.ts", "a\n");
    host.write("/repo/src/b.ts", "b\n");
    host.write("/repo/src/c.md", "c\n");
    host.write("/repo/gen/d.ts", "d\n");
    host.write("/repo/.gitignore", "gen/\n");
    host.symlink("/repo/link.ts", "/outside/e.ts");
    host.write("/outside/e.ts", "e\n");
  });

  const run = (dispatch: JssgDispatch, op: JssgOperation = operation) =>
    executeJssgCommand({
      cwd: "/repo",
      artifacts: new Map([[built.hash, built]]),
      commandId: "migrate",
      operation: op,
      dispatch,
      host,
      globalExcludes: null,
    });

  it("selects through the host, dispatches one batch, and commits through the host", async () => {
    const seen: OperationRequest[] = [];
    const completion = await run(upper(seen));
    expect(completion).toEqual({
      protocolVersion: PROTOCOL_VERSION,
      commandId: "migrate",
      status: "succeeded",
      output: [{ file: "a.ts" }, { file: "src/b.ts" }],
    });
    expect(seen).toHaveLength(1);
    expect(seen[0]!.context).toEqual({
      targetRoot: "/repo",
      files: [
        { path: "a.ts", content: "a\n" },
        { path: "src/b.ts", content: "b\n" },
      ],
      artifact: { source: built.source },
    });
    expect(host.files()).toMatchObject({
      "/repo/a.ts": "A\n",
      "/repo/src/b.ts": "B\n",
      "/repo/gen/d.ts": "d\n",
      "/outside/e.ts": "e\n",
    });
  });

  it("honors a target root and invocation globs", async () => {
    const seen: OperationRequest[] = [];
    await run(upper(seen), { ...operation, target: { root: "src", include: ["**/*.ts"] } });
    expect(seen[0]!.context!.targetRoot).toBe("/repo/src");
    expect(seen[0]!.context!.files).toEqual([{ path: "b.ts", content: "b\n" }]);
  });

  it("writes nothing when an edit escapes the root or two files conflict", async () => {
    const before = host.files();
    // `link.ts` is a symlink out of the root: containment resolves it through the host.
    const escape = await run(upper([], () => [{ path: "link.ts", content: "x" }]));
    expect(escape).toMatchObject({
      status: "failed",
      error: {
        message: "edited file 'link.ts' escapes the target root",
        details: { phase: "stage" },
      },
    });
    const conflict = await run(upper([], () => [{ path: "shared.ts", content: "x" }]));
    expect(conflict).toMatchObject({
      status: "failed",
      error: { message: "'shared.ts' is written by both 'a.ts' and 'src/b.ts'" },
    });
    expect(host.files()).toEqual(before);
    expect(host.calls.filter((call) => call.startsWith("writeText"))).toEqual([]);
  });

  it("turns a throwing dispatcher into a transform failure", async () => {
    const completion = await run(async () => {
      throw new Error("runner exploded");
    });
    expect(completion).toMatchObject({
      status: "failed",
      error: { message: "runner exploded", details: { phase: "transform" } },
    });
  });
});

describe("executeShell over the Node host", () => {
  let dir: string;
  beforeEach(() => {
    dir = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-shell-")));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  const request = (command: string, env?: Record<string, string>): OperationRequest => ({
    protocolVersion: PROTOCOL_VERSION,
    commandId: "cmd",
    operation: { kind: "shell", command, ...(env === undefined ? {} : { env }) },
  });

  it("normalizes output the way DirectRunner collects lines", () => {
    expect(normalizeShellOutput("")).toBe("");
    expect(normalizeShellOutput("a")).toBe("a\n");
    expect(normalizeShellOutput("a\r\nb\n")).toBe("a\nb\n");
    expect(normalizeShellOutput("\n\n")).toBe("\n\n");
  });

  it.skipIf(process.platform === "win32")(
    "succeeds with merged output in cwd and the layered environment",
    async () => {
      const completion = await executeShell(
        { cwd: dir, env: { FROM_EXECUTOR: "exec", BOTH: "executor" } },
        request('pwd; echo "$FROM_EXECUTOR $BOTH $FROM_OP"; echo err >&2', {
          BOTH: "operation",
          FROM_OP: "op",
        }),
      );
      expect(completion).toEqual({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "cmd",
        status: "succeeded",
        output: { stdout: `${dir}\nexec operation op\nerr\n` },
      });
    },
  );

  it.skipIf(process.platform === "win32")("fails with the exit code and output", async () => {
    const completion = await executeShell({ cwd: dir }, request("echo nope; exit 3"));
    expect(completion).toEqual({
      protocolVersion: PROTOCOL_VERSION,
      commandId: "cmd",
      status: "failed",
      error: { message: "Command failed with exit code 3: nope\n", exitCode: 3, output: "nope\n" },
    });
  });

  it.skipIf(process.platform === "win32")(
    "runs a shebang script from a temporary file",
    async () => {
      const completion = await executeShell({ cwd: dir }, request("#!/bin/sh\necho from-script\n"));
      expect(completion).toMatchObject({
        status: "succeeded",
        output: { stdout: "from-script\n" },
      });
    },
  );

  it.skipIf(process.platform === "win32")(
    "is cancelled by an abort, before or during",
    async () => {
      const before = new AbortController();
      before.abort();
      expect(await executeShell({ cwd: dir }, request("true"), before.signal)).toMatchObject({
        status: "cancelled",
        error: { message: "aborted before start" },
      });
      const during = new AbortController();
      const pending = executeShell({ cwd: dir }, request("sleep 30"), during.signal);
      setTimeout(() => during.abort(), 50);
      expect(await pending).toMatchObject({ status: "cancelled" });
    },
  );
});

describe("the Node host", () => {
  let dir: string;
  beforeEach(() => {
    dir = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-host-")));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it("reports invalid UTF-8 as EILSEQ only when fatal", () => {
    writeFileSync(join(dir, "bad.txt"), Uint8Array.from([0x61, 0xff, 0x62]));
    expect(NODE_HOST.fs.readText(join(dir, "bad.txt"))).toBe("a�b");
    expect(() => NODE_HOST.fs.readText(join(dir, "bad.txt"), { fatal: true })).toThrow(
      expect.objectContaining({ code: "EILSEQ" }),
    );
    expect(() => NODE_HOST.fs.readText(join(dir, "missing.txt"))).toThrow(
      expect.objectContaining({ code: "ENOENT" }),
    );
  });

  it.skipIf(process.platform === "win32")("types symlinks as neither file nor directory", () => {
    mkdirSync(join(dir, "sub"));
    writeFileSync(join(dir, "file"), "");
    symlinkSync(join(dir, "sub"), join(dir, "link"));
    const entries = NODE_HOST.fs.readdir(dir).sort((a, b) => a.name.localeCompare(b.name));
    expect(entries).toEqual([
      { name: "file", isFile: true, isDirectory: false },
      { name: "link", isFile: false, isDirectory: false },
      { name: "sub", isFile: false, isDirectory: true },
    ]);
    expect(NODE_HOST.fs.lstat(join(dir, "link")).isSymbolicLink).toBe(true);
  });
});

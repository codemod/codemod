/**
 * Bridge exchange placement and response reading: a host-owned per-user
 * root outside the target, private per-request directories, and responses
 * that are never read through symlinks or from non-regular files.
 */
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  BridgeExecutor,
  EXCHANGE_DIR_ENV,
  PROTOCOL_VERSION,
  exchangeRootCandidates,
  readResponse,
  resolveExchangeRoot,
  type OperationRequest,
} from "../src/index.ts";

const fakeBridge = resolve(import.meta.dirname, "fixtures/fake-bridge.mjs");
const posix = process.platform !== "win32";
const uid = typeof process.getuid === "function" ? process.getuid() : undefined;

describe("exchange root", () => {
  let base: string;
  let target: string;
  beforeEach(() => {
    base = realpathSync(mkdtempSync(join(tmpdir(), "exchange-test-")));
    target = join(base, "target");
    mkdirSync(target);
  });
  afterEach(() => rmSync(base, { recursive: true, force: true }));

  const host = (
    env: Record<string, string | undefined>,
    platform: NodeJS.Platform = process.platform,
  ) => ({
    env,
    platform,
    home: join(base, "home"),
    tmp: join(base, "tmp"),
    uid,
  });

  it("prefers the explicit directory, then the per-user runtime or cache directory, then tmp", () => {
    const home = join(base, "home");
    const tmp = join(base, "tmp");
    expect(exchangeRootCandidates({ ...host({}), platform: "darwin", uid: 501 })).toEqual([
      join(home, "Library", "Caches", "codemod", "bridge"),
      join(tmp, "codemod-bridge-501"),
    ]);
    expect(
      exchangeRootCandidates({
        ...host({
          XDG_RUNTIME_DIR: "/run/user/1000",
          XDG_CACHE_HOME: "/c",
          [EXCHANGE_DIR_ENV]: "/x",
        }),
        platform: "linux",
        uid: 1000,
      }),
    ).toEqual([
      "/x",
      "/run/user/1000/codemod/bridge",
      "/c/codemod/bridge",
      join(tmp, "codemod-bridge-1000"),
    ]);
    // Relative values are ignored.
    expect(
      exchangeRootCandidates({
        ...host({ XDG_RUNTIME_DIR: "run", [EXCHANGE_DIR_ENV]: "x" }),
        platform: "linux",
        uid: 1,
      }),
    ).toEqual([join(home, ".cache", "codemod", "bridge"), join(tmp, "codemod-bridge-1")]);
  });

  it.skipIf(!posix)(
    "creates a private root and refuses unsafe explicit roots without falling back",
    () => {
      const explicit = join(base, "exchange");
      const located = resolveExchangeRoot(target, host({ [EXCHANGE_DIR_ENV]: explicit }));
      expect(located).toEqual({ root: explicit });
      expect(statSync(explicit).mode & 0o777).toBe(0o700);

      // Owned but loose: tightened.
      chmodSync(explicit, 0o755);
      expect(resolveExchangeRoot(target, host({ [EXCHANGE_DIR_ENV]: explicit }))).toEqual({
        root: explicit,
      });
      expect(statSync(explicit).mode & 0o777).toBe(0o700);

      // Inside the target: refused, and the defaults are not tried instead.
      const inside = join(target, ".codemod-exchange");
      const refused = resolveExchangeRoot(target, host({ [EXCHANGE_DIR_ENV]: inside }));
      expect(refused).toEqual({
        problem: expect.stringContaining("is inside the bridge working directory"),
      });
      expect(existsSync(join(base, "home"))).toBe(false);

      // A symlink to an acceptable directory: refused.
      const link = join(base, "link");
      symlinkSync(explicit, link);
      expect(resolveExchangeRoot(target, host({ [EXCHANGE_DIR_ENV]: link }))).toEqual({
        problem: expect.stringContaining("is a symbolic link"),
      });
    },
  );

  it.skipIf(!posix || uid === 0)(
    "skips a default root it owns but cannot write, and keeps an explicit one strict",
    () => {
      const locked = join(base, "home", "Library", "Caches", "codemod", "bridge");
      mkdirSync(locked, { recursive: true, mode: 0o700 });
      chmodSync(locked, 0o500);
      try {
        const fallback = join(base, "tmp", `codemod-bridge-${uid}`);
        expect(resolveExchangeRoot(target, host({}, "darwin"))).toEqual({ root: fallback });
        expect(resolveExchangeRoot(target, host({ [EXCHANGE_DIR_ENV]: locked }, "darwin"))).toEqual(
          { problem: expect.stringContaining("is not writable") },
        );
      } finally {
        chmodSync(locked, 0o700);
      }
    },
  );

  it("reads only regular, bounded, non-symlink responses", () => {
    const file = join(base, "response.json");
    writeFileSync(file, "{}");
    expect(readResponse(file)).toEqual({ kind: "text", text: "{}" });
    expect(readResponse(join(base, "absent.json"))).toEqual({ kind: "missing" });
    expect(readResponse(file, 1)).toEqual({
      kind: "rejected",
      reason: "response is larger than 1 bytes",
    });
    const dir = join(base, "dir.json");
    mkdirSync(dir);
    expect(readResponse(dir)).toEqual({
      kind: "rejected",
      reason: "response is not a regular file",
    });
    const link = join(base, "link.json");
    symlinkSync(file, link);
    expect(readResponse(link)).toEqual({ kind: "rejected", reason: "response is a symbolic link" });
  });
});

describe("bridge exchange against an adversarial bridge", () => {
  let base: string;
  let target: string;
  let root: string;
  let previous: string | undefined;
  beforeEach(() => {
    base = realpathSync(mkdtempSync(join(tmpdir(), "exchange-bridge-")));
    target = join(base, "target");
    root = join(base, "exchange");
    mkdirSync(target);
    previous = process.env[EXCHANGE_DIR_ENV];
    process.env[EXCHANGE_DIR_ENV] = root;
  });
  afterEach(() => {
    if (previous === undefined) delete process.env[EXCHANGE_DIR_ENV];
    else process.env[EXCHANGE_DIR_ENV] = previous;
    rmSync(base, { recursive: true, force: true });
  });

  const request: OperationRequest = {
    protocolVersion: PROTOCOL_VERSION,
    commandId: "fix",
    operation: {
      kind: "agent",
      prompt: "x",
      backend: { kind: "codex", sandbox: "workspace-write" },
    },
  };

  it.each([
    ["symlink-response", "bridge response rejected: response is a symbolic link"],
    ["directory-response", "bridge response rejected: response is not a regular file"],
    ["garbage-response", /^executor returned invalid JSON/],
  ] as const)("does not trust a %s", async (mode, message) => {
    const sentinel = join(base, "outside-sentinel.json");
    const executor = new BridgeExecutor({
      bin: fakeBridge,
      cwd: target,
      env: { FAKE_BRIDGE_MODE: mode, FAKE_SENTINEL: sentinel },
    });
    const completion = await executor.execute(request);
    expect(completion.status).toBe("unknown");
    expect(completion.error?.message).toMatch(message);
    expect(completion.error?.details).toEqual({ phase: "bridge", repositoryMayBeModified: true });
    expect(JSON.stringify(completion)).not.toContain("forged");
    if (mode === "symlink-response") {
      // The link target was neither followed for reading nor removed by cleanup.
      expect(JSON.parse(readFileSync(sentinel, "utf8")).output).toEqual({ text: "forged" });
    }
    // Every per-request exchange directory is cleaned up; nothing is left in the target.
    expect(readdirSync(root)).toEqual([]);
    expect(readdirSync(target)).toEqual([]);
  });

  it("keeps exchange files out of the target and private while the bridge runs", async () => {
    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: target });
    const completion = await executor.execute({
      ...request,
      operation: { kind: "agent", prompt: "x", backend: { kind: "builtin", tools: [] } },
    });
    expect(completion.status).toBe("succeeded");
    expect(readdirSync(target)).toEqual([]);
    expect(readdirSync(root)).toEqual([]);
    if (posix) expect(statSync(root).mode & 0o777).toBe(0o700);
  });

  it("refuses to start when the exchange would live inside the target", async () => {
    process.env[EXCHANGE_DIR_ENV] = join(target, "exchange");
    const completion = await new BridgeExecutor({ bin: fakeBridge, cwd: target }).execute(request);
    expect(completion).toMatchObject({
      status: "failed",
      error: {
        message: expect.stringContaining("is inside the bridge working directory"),
        details: { phase: "config", repositoryMayBeModified: false },
      },
    });
  });
});

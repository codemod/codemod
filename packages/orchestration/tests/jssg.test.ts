/**
 * The TypeScript JSSG orchestrator against the scripted fake worker
 * (`fixtures/fake-bridge.mjs --jssg-worker`): selection, indexing order,
 * staging, transactional commit, failure classification, path validation of
 * worker output, and cancellation. No Rust is involved; the real worker is
 * exercised by `bridge.e2e.test.ts`.
 */
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  CollectingSink,
  executeJssg,
  type JssgOperation,
  type WorkflowEvent,
} from "../src/index.ts";

const bin = resolve(import.meta.dirname, "fixtures/fake-bridge.mjs");

let repo: string;
const write = (relative: string, content: string) => {
  const path = join(repo, relative);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
};
const read = (relative: string) => readFileSync(join(repo, relative), "utf8");
const snapshot = () =>
  Object.fromEntries(
    ["a.ts", "b.ts", "c.js", "d.md"]
      .filter((f) => existsSync(join(repo, f)))
      .map((f) => [f, read(f)]),
  );

beforeEach(() => {
  chmodSync(bin, 0o755);
  repo = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-jssg-")));
  write("a.ts", "a\n");
  write("b.ts", "b\n");
  write("c.js", "c\n");
  write("d.md", "d\n");
});
afterEach(() => rmSync(repo, { recursive: true, force: true }));

const operation = (extra: Partial<JssgOperation> = {}): JssgOperation => ({
  kind: "jssg",
  script: "transform.ts",
  language: "typescript",
  ...extra,
});

function run(
  op: JssgOperation,
  options: { mode?: string; signal?: AbortSignal; secondary?: string } = {},
) {
  const events = new CollectingSink();
  const env: Record<string, string> = {};
  if (options.mode) env.FAKE_BRIDGE_MODE = options.mode;
  if (options.secondary) env.FAKE_SECONDARY_PATH = options.secondary;
  return executeJssg({
    bin,
    cwd: repo,
    scriptRoot: join(repo, "workflow"),
    commandId: "migrate",
    operation: op,
    signal: options.signal,
    events,
    env,
    globalExcludes: null,
  }).then((completion) => ({ completion, events: events.events }));
}

describe("executeJssg", () => {
  it("uses one worker, selects by worker-reported extensions, indexes before transforming, and commits", async () => {
    const { completion, events } = await run(
      operation({ semanticAnalysis: "workspace", input: { n: 1 } }),
    );

    expect(completion.status).toBe("succeeded");
    const outputs = completion.output as {
      path: string;
      open: Record<string, unknown>;
      indexed: string[];
    }[];
    expect(outputs.map((o) => o.path)).toEqual(["a.ts", "b.ts", "c.js"]);
    // The whole selected set was indexed before the first transform.
    expect(outputs[0]!.indexed).toEqual(["a.ts", "b.ts", "c.js"]);
    // Staged edits are re-indexed after each transform.
    expect(outputs[1]!.indexed).toEqual(["a.ts", "b.ts", "c.js", "a.ts"]);
    expect(outputs[0]!.open).toEqual({
      script: "transform.ts",
      scriptRoot: join(repo, "workflow"),
      language: "typescript",
      targetRoot: repo,
      semanticAnalysis: "workspace",
      input: { n: 1 },
    });
    expect(snapshot()).toEqual({
      "a.ts": "a\n// fake\n",
      "b.ts": "b\n// fake\n",
      "c.js": "c\n// fake\n",
      "d.md": "d\n",
    });
    expect(events.filter((e) => e.type === "jssg.worker")).toHaveLength(1);
    const phases = events
      .filter(
        (e): e is Extract<WorkflowEvent, { type: "jssg.progress" }> => e.type === "jssg.progress",
      )
      .map((e) => e.phase);
    expect(phases).toEqual([
      "select",
      "index",
      "index",
      "index",
      "transform",
      "transform",
      "transform",
      "commit",
      "commit",
    ]);
  });

  it("intersects definition globs (repository-relative) with the invocation target", async () => {
    write("app/src/x.ts", "x\n");
    write("app/src/y.generated.ts", "y\n");
    write("app/other/z.ts", "z\n");
    const { completion } = await run(
      operation({
        include: ["app/**/*.ts"],
        target: { root: "app", include: ["src/**"], exclude: ["**/*.generated.ts"] },
      }),
    );
    expect(completion.status).toBe("succeeded");
    expect((completion.output as { path: string }[]).map((o) => o.path)).toEqual(["src/x.ts"]);
    expect(read("app/src/x.ts")).toBe("x\n// fake\n");
    expect(read("app/src/y.generated.ts")).toBe("y\n");
    expect(read("app/other/z.ts")).toBe("z\n");
    expect(read("a.ts")).toBe("a\n");
  });

  it("does not index in file-scope or no-semantics mode before transforms", async () => {
    const { completion } = await run(operation({ semanticAnalysis: "file" }));
    const outputs = completion.output as { indexed: string[] }[];
    expect(outputs[0]!.indexed).toEqual([]);
    expect(outputs[1]!.indexed).toEqual(["a.ts"]);
    const plain = await run(operation());
    expect((plain.completion.output as { indexed: string[] }[])[2]!.indexed).toEqual([]);
  });

  it("fails without changing any file when a later transform fails", async () => {
    const before = snapshot();
    const { completion } = await run(operation(), { mode: "fail-second" });
    expect(completion.status).toBe("failed");
    expect(completion.error?.message).toMatch(/scripted transform failure/);
    expect(completion.error?.details).toEqual({ phase: "transform", path: "b.ts", fatal: false });
    expect(snapshot()).toEqual(before);
  });

  it("fails when the worker dies with a fatal error", async () => {
    const { completion } = await run(operation(), { mode: "fatal" });
    expect(completion.status).toBe("failed");
    expect(completion.error?.details).toMatchObject({
      phase: "transform",
      path: "a.ts",
      fatal: true,
    });
    expect(read("a.ts")).toBe("a\n");
  });

  it("rejects worker output that escapes the target root at the protocol boundary", async () => {
    const before = snapshot();
    // `..` and absolute forms never pass response validation.
    const escape = await run(operation(), { mode: "escape" });
    expect(escape.completion.status).toBe("failed");
    expect(escape.completion.error?.details).toMatchObject({ phase: "transform", path: "a.ts" });
    expect(escape.completion.error?.message).toMatch(/invalid message/);
    expect(existsSync(join(dirname(repo), "escaped.ts"))).toBe(false);

    const secondary = await run(operation(), { mode: "escape-secondary" });
    expect(secondary.completion.status).toBe("failed");
    expect(secondary.completion.error?.message).toMatch(/invalid message/);
    expect(snapshot()).toEqual(before);
  });

  it.skipIf(process.platform === "win32")(
    "rejects worker output that escapes through a symlink before staging",
    async () => {
      const outside = mkdtempSync(join(tmpdir(), "codemod-outside-"));
      try {
        mkdirSync(join(outside, "dir"));
        symlinkSync(join(outside, "dir"), join(repo, "linkdir"));
        const before = snapshot();
        const { completion } = await run(operation(), { mode: "escape-symlink" });
        expect(completion.status).toBe("failed");
        expect(completion.error?.details).toMatchObject({ phase: "stage", path: "a.ts" });
        expect(completion.error?.message).toMatch(/escapes the target root/);
        expect(existsSync(join(outside, "dir", "out.ts"))).toBe(false);
        expect(snapshot()).toEqual(before);
      } finally {
        rmSync(outside, { recursive: true, force: true });
      }
    },
  );

  it("rejects conflicting destinations before writing", async () => {
    const before = snapshot();
    const { completion } = await run(operation(), { mode: "conflict" });
    expect(completion.status).toBe("failed");
    expect(completion.error?.details).toEqual({
      phase: "stage",
      path: "same.ts",
      origin: "b.ts",
      conflictingOrigin: "a.ts",
    });
    expect(snapshot()).toEqual(before);
    expect(existsSync(join(repo, "same.ts"))).toBe(false);
  });

  it("commits renames: destinations written, sources removed afterwards", async () => {
    const { completion } = await run(operation({ include: ["**/*.ts"] }), { mode: "rename" });
    expect(completion.status).toBe("succeeded");
    expect(read("a.moved.ts")).toBe("a\n");
    expect(read("b.moved.ts")).toBe("b\n");
    expect(existsSync(join(repo, "a.ts"))).toBe(false);
    expect(existsSync(join(repo, "b.ts"))).toBe(false);
    expect(read("c.js")).toBe("c\n");
  });

  it("chains a secondary edit into a later file's transform input", async () => {
    const { completion } = await run(operation({ include: ["**/*.ts"] }), {
      mode: "secondary",
      secondary: "b.ts",
    });
    expect(completion.status).toBe("succeeded");
    expect(read("a.ts")).toBe("a\n// primary a.ts\n");
    expect(read("b.ts")).toBe("// secondary\n// primary b.ts\n");
  });

  it("skips files that vanished or are not UTF-8", async () => {
    writeFileSync(join(repo, "b.ts"), Buffer.from([0xff, 0xfe, 0x62]));
    const { completion } = await run(operation({ include: ["**/*.ts"] }));
    expect(completion.status).toBe("succeeded");
    expect((completion.output as { path: string }[]).map((o) => o.path)).toEqual(["a.ts"]);
  });

  it("reports selection errors and unresolvable target roots as failed", async () => {
    const bad = await run(operation({ include: ["["] }));
    expect(bad.completion.status).toBe("failed");
    expect(bad.completion.error?.details).toEqual({ phase: "select" });
    const missing = await run(operation({ target: { root: "nope" } }));
    expect(missing.completion.status).toBe("failed");
    expect(missing.completion.error?.message).toMatch(/target root/);
  });

  it.skipIf(process.platform === "win32")(
    "rejects a target root that is a symlink out of the repository",
    async () => {
      const outside = mkdtempSync(join(tmpdir(), "codemod-outside-"));
      try {
        writeFileSync(join(outside, "secret.ts"), "secret\n");
        symlinkSync(outside, join(repo, "link"));
        const { completion } = await run(operation({ target: { root: "link" } }));
        expect(completion.status).toBe("failed");
        expect(completion.error?.message).toMatch(/escapes the target root/);
        expect(readFileSync(join(outside, "secret.ts"), "utf8")).toBe("secret\n");
      } finally {
        rmSync(outside, { recursive: true, force: true });
      }
    },
  );

  it("cancels before commit, kills the worker, and leaves files untouched", async () => {
    const before = snapshot();
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 200);
    const { completion, events } = await run(operation(), {
      mode: "hang",
      signal: controller.signal,
    });
    expect(completion.status).toBe("cancelled");
    expect(completion.error?.details).toMatchObject({ committed: false });
    expect(snapshot()).toEqual(before);
    const pid = (events.find((e) => e.type === "jssg.worker") as { pid?: number }).pid!;
    await expectGone(pid);
  });

  it.skipIf(process.platform === "win32" || process.getuid?.() === 0)(
    "reports unknown with structured details when the commit fails part-way",
    async () => {
      write("ro/e.ts", "e\n");
      chmodSync(join(repo, "ro"), 0o555);
      try {
        const { completion } = await run(operation({ include: ["**/*.ts"] }));
        expect(completion.status).toBe("unknown");
        expect(completion.error?.details).toMatchObject({
          phase: "commit",
          applied: ["a.ts", "b.ts"],
          failed: "ro/e.ts",
          remaining: ["ro/e.ts"],
          aborted: false,
        });
        expect(read("a.ts")).toBe("a\n// fake\n");
        expect(read("ro/e.ts")).toBe("e\n");
      } finally {
        chmodSync(join(repo, "ro"), 0o755);
      }
    },
  );
});

async function expectGone(pid: number): Promise<void> {
  for (let attempt = 0; attempt < 50; attempt++) {
    try {
      process.kill(pid, 0);
    } catch {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`worker ${pid} is still alive`);
}

/**
 * The TypeScript JSSG orchestrator against the scripted fake bridge
 * (`fixtures/fake-bridge.mjs`): selection, the batch request, validation of
 * returned edits, conflict rules, transactional commit, failure
 * classification, and cancellation. No Rust is involved; the real bridge is
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
import { CollectingSink, executeJssg, type JssgOperation } from "../src/index.ts";

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
    ["a.ts", "b.ts", "c.js", "d.md", "same.ts", "b.moved.ts"]
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
  it("selects by language, sends one batch with the roots and input, commits, and collects outputs", async () => {
    const { completion, events } = await run(
      operation({ semanticAnalysis: "workspace", input: { n: 1 } }),
    );

    expect(completion.status).toBe("succeeded");
    expect(completion.output).toEqual(
      ["a.ts", "b.ts", "c.js"].map((path) => ({
        path,
        context: { scriptRoot: join(repo, "workflow"), targetRoot: repo },
        input: { n: 1 },
      })),
    );
    expect(snapshot()).toEqual({
      "a.ts": "a\n// fake\n",
      "b.ts": "b\n// fake\n",
      "c.js": "c\n// fake\n",
      "d.md": "d\n",
    });
    expect(events.filter((e) => e.type === "bridge.spawned")).toHaveLength(1);
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

  it.each<[string, { mode: string; include?: string[]; existing?: string }, string, RegExp]>([
    // (description, fake mode and selection, expected phase, expected message)
    ["the bridge fails", { mode: "fail" }, "transform", /scripted transform failure/],
    ["an edit renames outside the root", { mode: "escape" }, "transform", /invalid batch result/],
    [
      "an edit targets an absolute path",
      { mode: "escape-absolute" },
      "transform",
      /invalid batch result/,
    ],
    [
      "two edits rename onto one destination",
      { mode: "conflict" },
      "stage",
      /'same.ts' is written by both 'a.ts' and 'b.ts'/,
    ],
    [
      "one source is renamed twice",
      { mode: "rename-twice", include: ["a.ts", "b.ts"] },
      "stage",
      /'b.ts' is renamed by both 'a.ts' and 'b.ts'/,
    ],
    [
      "an edit writes a path another edit renames away",
      { mode: "write-renamed", include: ["a.ts", "b.ts"] },
      "stage",
      /'b.ts' is renamed away by 'a.ts' and written by 'b.ts'/,
    ],
    [
      "a rename lands on an existing file",
      { mode: "rename", include: ["a.ts", "b.ts"], existing: "b.moved.ts" },
      "stage",
      /'b.ts' renames onto 'b.moved.ts', which already exists/,
    ],
  ])("changes nothing when %s", async (_name, { mode, include, existing }, phase, message) => {
    if (existing) write(existing, "existing\n");
    const before = snapshot();
    const { completion } = await run(operation({ include }), { mode });
    expect(completion.status).toBe("failed");
    expect(completion.error?.message).toMatch(message);
    expect(completion.error?.details).toEqual({ phase });
    expect(snapshot()).toEqual(before);
    expect(existsSync(join(repo, "same.ts"))).toBe(false);
  });

  it.skipIf(process.platform === "win32")(
    "rejects an edit that escapes through a symlink before writing",
    async () => {
      const outside = mkdtempSync(join(tmpdir(), "codemod-outside-"));
      try {
        mkdirSync(join(outside, "dir"));
        symlinkSync(join(outside, "dir"), join(repo, "linkdir"));
        const before = snapshot();
        const { completion } = await run(operation(), { mode: "escape-symlink" });
        expect(completion.status).toBe("failed");
        expect(completion.error?.details).toEqual({ phase: "stage" });
        expect(completion.error?.message).toMatch(/escapes the target root/);
        expect(existsSync(join(outside, "dir", "out.ts"))).toBe(false);
        expect(snapshot()).toEqual(before);
      } finally {
        rmSync(outside, { recursive: true, force: true });
      }
    },
  );

  it("commits renames and secondary edits: destinations written, sources removed afterwards", async () => {
    const renamed = await run(operation({ include: ["**/*.ts"] }), { mode: "rename" });
    expect(renamed.completion.status).toBe("succeeded");
    expect(read("a.moved.ts")).toBe("a\n");
    expect(read("b.moved.ts")).toBe("b\n");
    expect(existsSync(join(repo, "a.ts"))).toBe(false);
    expect(existsSync(join(repo, "b.ts"))).toBe(false);
    expect(read("c.js")).toBe("c\n");

    write("a.ts", "a\n");
    const secondary = await run(operation({ include: ["a.ts"] }), {
      mode: "secondary",
      secondary: "new/dir/n.ts",
    });
    expect(secondary.completion.status).toBe("succeeded");
    expect(read("a.ts")).toBe("a\n// primary a.ts\n");
    expect(read("new/dir/n.ts")).toBe("// secondary\n");
  });

  it("skips files that vanished or are not UTF-8", async () => {
    writeFileSync(join(repo, "b.ts"), Buffer.from([0xff, 0xfe, 0x62]));
    const { completion } = await run(operation({ include: ["**/*.ts"] }));
    expect(completion.status).toBe("succeeded");
    expect((completion.output as { path: string }[]).map((o) => o.path)).toEqual(["a.ts"]);
  });

  it.skipIf(process.platform === "win32")(
    "reports unresolvable or escaping target roots as select failures",
    async () => {
      const missing = await run(operation({ target: { root: "nope" } }));
      expect(missing.completion.status).toBe("failed");
      expect(missing.completion.error?.details).toEqual({ phase: "select" });
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

  it("cancels before commit, kills the bridge, and leaves files untouched", async () => {
    const before = snapshot();
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 200);
    const { completion, events } = await run(operation(), {
      mode: "hang",
      signal: controller.signal,
    });
    expect(completion.status).toBe("cancelled");
    expect(completion.error?.details).toEqual({ phase: "transform" });
    expect(snapshot()).toEqual(before);
    const pid = (events.find((e) => e.type === "bridge.spawned") as { pid?: number }).pid!;
    await expectGone(pid);
  });

  it.skipIf(process.platform === "win32" || process.getuid?.() === 0)(
    "reports unknown with structured details when the commit fails part-way",
    async () => {
      write("ro/e.ts", "e\n");
      chmodSync(join(repo, "ro/e.ts"), 0o444);
      const { completion } = await run(operation({ include: ["**/*.ts"] }));
      expect(completion.status).toBe("unknown");
      expect(completion.error?.details).toEqual({
        phase: "commit",
        applied: ["a.ts", "b.ts"],
        failed: "ro/e.ts",
        remaining: ["ro/e.ts"],
      });
      expect(read("a.ts")).toBe("a\n// fake\n");
      expect(read("ro/e.ts")).toBe("e\n");
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
  throw new Error(`bridge ${pid} is still alive`);
}

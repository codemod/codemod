/**
 * `executeJssgCommand` against a scripted batch dispatch. No Rust is involved;
 * the real batch runners are exercised by the runtimes' e2e tests.
 */
import {
  chmodSync,
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
import { dirname, join, sep } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { executeJssgCommand, type JssgDispatch } from "../src/execution/jssg.ts";
import {
  PROTOCOL_VERSION,
  type FileOutcome,
  type Json,
  type JssgOperation,
  type OperationCompletion,
} from "../src/index.ts";
import { artifact } from "./helpers.ts";

const built = artifact("transform", "export default () => null;\n");

let repo: string;
const write = (relative: string, content: string) => {
  const path = join(repo, relative);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
};
const read = (relative: string) => readFileSync(join(repo, relative), "utf8");
/** Every file in the repository, by repository-relative path. */
const tree = () =>
  Object.fromEntries(
    readdirSync(repo, { recursive: true, encoding: "utf8" })
      .filter((f) => statSync(join(repo, f)).isFile())
      .sort()
      .map((f) => [f.split(sep).join("/"), read(f)]),
  );
const untouched = { "a.ts": "a\n", "b.ts": "b\n", "c.js": "c\n", "d.md": "d\n" };

const createRepo = () => {
  repo = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-jssg-")));
  for (const [path, content] of Object.entries(untouched)) write(path, content);
};
const outsideDirs: string[] = [];
const outsideDir = () => {
  const dir = mkdtempSync(join(tmpdir(), "codemod-outside-"));
  outsideDirs.push(dir);
  return dir;
};
const removeDirs = () => {
  for (const dir of [repo, ...outsideDirs.splice(0)]) rmSync(dir, { recursive: true, force: true });
};

const operation = (extra: Partial<JssgOperation> = {}): JssgOperation => ({
  kind: "jssg",
  transform: { name: built.name, hash: built.hash },
  language: "typescript",
  ...extra,
});

type Edits = FileOutcome["edits"];
const edit = (path: string, content: string, renameTo?: string) =>
  renameTo === undefined ? { path, content } : { path, content, renameTo };

/** Scripted edits per mode for one file; `secondary` is the path a.ts also writes. */
const modes: Record<string, (path: string, content: string, secondary?: string) => Edits> = {
  escape: (path, content) => [edit(path, content, "../escaped.ts")],
  "escape-symlink": (path, content) => [edit(path, content, "linkdir/out.ts")],
  "escape-absolute": () => [edit("/etc/passwd", "pwned")],
  conflict: (path, content) => [edit(path, content, "same.ts")],
  rename: (path, content) => [edit(path, content, path.replace(/\.ts$/u, ".moved.ts"))],
  "rename-twice": (path, content) =>
    path === "a.ts"
      ? [edit(path, content), edit("b.ts", "b\n", "x.ts")]
      : [edit(path, content, "y.ts")],
  "write-renamed": (path, content) =>
    path === "a.ts"
      ? [edit(path, content), edit("b.ts", "b\n", "b.renamed.ts")]
      : [edit(path, `${content}// b\n`)],
  secondary: (path, content, secondary) => [
    edit(path, `${content}// primary ${path}\n`),
    ...(path === "a.ts" ? [edit(secondary!, "// secondary\n")] : []),
  ],
};

/**
 * A scripted batch runner. `fail` fails the batch and `hang` answers only once
 * aborted; `modes` script edits; any other mode appends `// fake\n` to every
 * file and outputs `{ path, context, input }` (the request context without files).
 */
function scriptedDispatch(mode: string, secondary?: string): JssgDispatch {
  return async (request, signal) => {
    const reply = (completion: Omit<OperationCompletion, "protocolVersion" | "commandId">) =>
      ({
        protocolVersion: PROTOCOL_VERSION,
        commandId: request.commandId,
        ...completion,
      }) as OperationCompletion;
    if (mode === "fail") {
      return reply({ status: "failed", error: { message: "scripted transform failure" } });
    }
    if (mode === "hang") {
      await new Promise((resolve) => signal?.addEventListener("abort", resolve, { once: true }));
      return reply({ status: "cancelled", error: { message: "aborted" } });
    }
    const { files = [], ...context } = request.context ?? {};
    const input = (request.operation as JssgOperation).input ?? null;
    const outcomes = files.map(
      ({ path, content }): FileOutcome =>
        mode in modes
          ? { path, edits: modes[mode]!(path, content, secondary) }
          : {
              path,
              edits: [edit(path, `${content}// fake\n`)],
              output: { path, context: context as Json, input },
            },
    );
    return reply({ status: "succeeded", output: { files: outcomes as unknown as Json } });
  };
}

/** `abort` fires the signal once the scripted dispatch has started (a hanging one is listening). */
function run(
  op: JssgOperation,
  options: { mode?: string; secondary?: string; abort?: boolean } = {},
) {
  let dispatched = 0;
  const controller = new AbortController();
  const dispatch = scriptedDispatch(options.mode ?? "echo", options.secondary);
  return executeJssgCommand({
    cwd: repo,
    artifacts: new Map([[built.hash, built]]),
    commandId: "migrate",
    operation: op,
    signal: controller.signal,
    dispatch: (request, signal) => {
      dispatched += 1;
      const completion = dispatch(request, signal);
      if (options.abort) controller.abort();
      return completion;
    },
    globalExcludes: null,
  }).then((completion) => ({ completion, dispatched }));
}

/** A failed completion, by the phase it failed in and its message. */
const failure = (phase: string, message = /./) =>
  expect.objectContaining({
    status: "failed",
    error: { message: expect.stringMatching(message), details: { phase } },
  });
const paths = (completion: OperationCompletion) =>
  (completion.output as { path: string }[]).map((o) => o.path);

describe("a successful batch", () => {
  let result: Awaited<ReturnType<typeof run>>;
  let after: Record<string, string>;
  beforeAll(async () => {
    createRepo();
    result = await run(operation({ semanticAnalysis: "workspace", input: { n: 1 } }));
    after = tree();
  });
  afterAll(removeDirs);

  it("sends every selected file in one batch", () => {
    expect(result.dispatched).toBe(1);
  });

  it("selects the files of the operation's language family", () => {
    expect(paths(result.completion)).toEqual(["a.ts", "b.ts", "c.js"]);
  });

  it("sends the target root, the artifact source, and the input with the batch", () => {
    expect((result.completion.output as Json[])[0]).toEqual({
      path: "a.ts",
      context: { targetRoot: repo, artifact: { source: built.source } },
      input: { n: 1 },
    });
  });

  it("commits the edits of selected files and leaves the others untouched", () => {
    expect(after).toEqual({
      "a.ts": "a\n// fake\n",
      "b.ts": "b\n// fake\n",
      "c.js": "c\n// fake\n",
      "d.md": "d\n",
    });
  });
});

describe("executeJssgCommand", () => {
  beforeEach(createRepo);
  afterEach(removeDirs);

  const intersected = () => {
    write("app/src/x.ts", "x\n");
    write("app/src/y.generated.ts", "y\n");
    write("app/other/z.ts", "z\n");
    return run(
      operation({
        include: ["app/**/*.ts"],
        target: { root: "app", include: ["src/**"], exclude: ["**/*.generated.ts"] },
      }),
    );
  };

  it("selects files matching both the repository-relative definition globs and the target's globs", async () => {
    const { completion } = await intersected();
    expect(paths(completion)).toEqual(["src/x.ts"]);
  });

  it("commits edits relative to the target root", async () => {
    await intersected();
    expect(read("app/src/x.ts")).toBe("x\n// fake\n");
  });

  const failsUnchanged = async (mode: string, phase: string, message: RegExp) => {
    // `rename` maps b.ts onto this existing file; no other mode touches it.
    write("b.moved.ts", "existing\n");
    const before = tree();
    const { completion } = await run(operation({ include: ["a.ts", "b.ts"] }), { mode });
    expect([completion, tree()]).toEqual([failure(phase, message), before]);
  };

  it.each([
    ["the batch fails", "fail", /scripted transform failure/],
    ["an edit renames outside the root", "escape", /invalid batch result/],
    ["an edit targets an absolute path", "escape-absolute", /invalid batch result/],
  ])("fails in the transform phase, changing nothing, when %s", (_name, mode, message) =>
    failsUnchanged(mode, "transform", message),
  );

  it.each([
    [
      "two edits rename onto one path",
      "conflict",
      /'same.ts' is written by both 'a.ts' and 'b.ts'/,
    ],
    ["one source is renamed twice", "rename-twice", /'b.ts' is renamed by both 'a.ts' and 'b.ts'/],
    [
      "a renamed-away path is written",
      "write-renamed",
      /renamed away by 'a.ts' and written by 'b.ts'/,
    ],
    ["a rename lands on an existing file", "rename", /'b.ts' renames onto 'b.moved.ts', which/],
  ])("fails in the stage phase, changing nothing, when %s", (_name, mode, message) =>
    failsUnchanged(mode, "stage", message),
  );

  it.skipIf(process.platform === "win32")(
    "rejects an edit that escapes through a symlink before writing",
    async () => {
      const outside = outsideDir();
      mkdirSync(join(outside, "dir"));
      symlinkSync(join(outside, "dir"), join(repo, "linkdir"));
      const before = tree();
      const { completion } = await run(operation(), { mode: "escape-symlink" });
      expect([completion, readdirSync(outside, { recursive: true }), tree()]).toEqual([
        failure("stage", /escapes the target root/),
        ["dir"],
        before,
      ]);
    },
  );

  it("renames a file: writes the destination and removes the source", async () => {
    await run(operation({ include: ["**/*.ts"] }), { mode: "rename" });
    const { "a.ts": a, "b.ts": b, ...rest } = untouched;
    expect(tree()).toEqual({ ...rest, "a.moved.ts": a, "b.moved.ts": b });
  });

  it("writes a secondary edit to a new path, creating its directories", async () => {
    await run(operation({ include: ["a.ts"] }), { mode: "secondary", secondary: "new/dir/n.ts" });
    expect(tree()).toEqual({
      ...untouched,
      "a.ts": "a\n// primary a.ts\n",
      "new/dir/n.ts": "// secondary\n",
    });
  });

  it("skips a file that is not UTF-8", async () => {
    writeFileSync(join(repo, "b.ts"), Buffer.from([0xff, 0xfe, 0x62]));
    const { completion } = await run(operation({ include: ["**/*.ts"] }));
    expect(paths(completion)).toEqual(["a.ts"]);
  });

  it("fails in the select phase when the target root does not exist", async () => {
    const { completion } = await run(operation({ target: { root: "nope" } }));
    expect(completion).toEqual(failure("select"));
  });

  it.skipIf(process.platform === "win32")(
    "fails in the select phase, without dispatching, when the target root escapes through a symlink",
    async () => {
      const outside = outsideDir();
      writeFileSync(join(outside, "secret.ts"), "secret\n");
      symlinkSync(outside, join(repo, "link"));
      const { completion, dispatched } = await run(operation({ target: { root: "link" } }));
      expect([completion, dispatched]).toEqual([failure("select", /escapes the target root/), 0]);
    },
  );

  it("reports cancelled in the transform phase and changes nothing when aborted mid-batch", async () => {
    const { completion } = await run(operation(), { mode: "hang", abort: true });
    expect([completion.status, completion.error?.details, tree()]).toEqual([
      "cancelled",
      { phase: "transform" },
      untouched,
    ]);
  });

  it.skipIf(process.platform === "win32" || process.getuid?.() === 0)(
    "reports unknown with what was applied and what remains when the commit fails part-way",
    async () => {
      write("ro/e.ts", "e\n");
      chmodSync(join(repo, "ro/e.ts"), 0o444);
      const { completion } = await run(operation({ include: ["**/*.ts"] }));
      expect([completion.status, completion.error?.details, tree()]).toEqual([
        "unknown",
        { phase: "commit", applied: ["a.ts", "b.ts"], failed: "ro/e.ts", remaining: ["ro/e.ts"] },
        { ...untouched, "a.ts": "a\n// fake\n", "b.ts": "b\n// fake\n", "ro/e.ts": "e\n" },
      ]);
    },
  );
});

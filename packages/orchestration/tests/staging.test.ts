/**
 * Staged edits: chaining, conflict detection, path containment, and the
 * commit that applies everything only after every result is staged.
 */
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  CommitError,
  PathEscapeError,
  Staging,
  StagingConflictError,
  resolveInsideRoot,
  type TransformResult,
} from "../src/index.ts";

const modified = (content: string, renameTo?: string): TransformResult => ({
  primary:
    renameTo === undefined
      ? { kind: "modified", content }
      : { kind: "modified", content, renameTo },
  secondary: [],
});
const unmodified: TransformResult = { primary: { kind: "unmodified" }, secondary: [] };

let root: string;
const write = (relative: string, content: string) => {
  const path = join(root, relative);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
};
const read = (relative: string) => readFileSync(join(root, relative), "utf8");

beforeEach(() => {
  root = realpathSync.native(mkdtempSync(join(tmpdir(), "codemod-staging-")));
  write("a.ts", "a\n");
  write("b.ts", "b\n");
  write("src/c.ts", "c\n");
});
afterEach(() => rmSync(root, { recursive: true, force: true }));

describe("Staging.apply", () => {
  it("stages primary and secondary edits without touching disk", () => {
    const staging = new Staging(root);
    const written = staging.apply("a.ts", {
      primary: { kind: "modified", content: "A\n" },
      secondary: [{ path: "src/c.ts", result: { kind: "modified", content: "C\n" } }],
    });
    expect(written).toEqual([
      { path: "a.ts", content: "A\n" },
      { path: "src/c.ts", content: "C\n" },
    ]);
    expect(staging.contentFor("a.ts")).toBe("A\n");
    expect(staging.contentFor("src/c.ts")).toBe("C\n");
    expect(staging.contentFor("b.ts")).toBeUndefined();
    expect(read("a.ts")).toBe("a\n");
    expect(read("src/c.ts")).toBe("c\n");
    expect(staging.apply("b.ts", unmodified)).toEqual([]);
  });

  it("chains an earlier secondary edit into the file's own later primary result", () => {
    const staging = new Staging(root);
    staging.apply("a.ts", {
      primary: { kind: "unmodified" },
      secondary: [{ path: "b.ts", result: { kind: "modified", content: "from a\n" } }],
    });
    expect(staging.contentFor("b.ts")).toBe("from a\n");
    staging.apply("b.ts", modified("from a\nfrom b\n"));
    expect(staging.contentFor("b.ts")).toBe("from a\nfrom b\n");
  });

  it("rejects two secondary edits of one file and leaves the staging untouched", () => {
    const staging = new Staging(root);
    staging.apply("a.ts", {
      primary: { kind: "unmodified" },
      secondary: [{ path: "src/c.ts", result: { kind: "modified", content: "from a\n" } }],
    });
    expect(() =>
      staging.apply("b.ts", {
        primary: { kind: "modified", content: "B\n" },
        secondary: [{ path: "src/c.ts", result: { kind: "modified", content: "from b\n" } }],
      }),
    ).toThrow(StagingConflictError);
    expect(staging.contentFor("src/c.ts")).toBe("from a\n");
    expect(staging.contentFor("b.ts")).toBeUndefined();
  });

  it("rejects duplicate rename destinations and renames onto existing files", () => {
    const staging = new Staging(root);
    staging.apply("a.ts", modified("a\n", "moved.ts"));
    let error = catchError(() => staging.apply("b.ts", modified("b\n", "moved.ts")));
    expect(error).toBeInstanceOf(StagingConflictError);
    expect((error as StagingConflictError).detail).toEqual({
      path: "moved.ts",
      origin: "b.ts",
      conflictingOrigin: "a.ts",
    });
    error = catchError(() => staging.apply("b.ts", modified("b\n", "src/c.ts")));
    expect((error as Error).message).toMatch(/already exists/);
    expect(staging.plan()).toEqual({ writes: ["moved.ts"], deletes: ["a.ts"] });
  });

  it("allows a rename onto a path that an earlier result renamed away, and skips renamed sources", () => {
    const staging = new Staging(root);
    staging.apply("a.ts", modified("a\n", "moved.ts"));
    expect(staging.isRemoved("a.ts")).toBe(true);
    staging.apply("b.ts", modified("b\n", "a.ts"));
    expect(staging.isRemoved("a.ts")).toBe(false);
    expect(staging.plan()).toEqual({ writes: ["a.ts", "moved.ts"], deletes: ["b.ts"] });
    expect(() => staging.apply("src/c.ts", modified("c\n", "b.ts"))).not.toThrow();
    expect(staging.plan()).toEqual({ writes: ["a.ts", "b.ts", "moved.ts"], deletes: ["src/c.ts"] });
  });

  it("rejects writes to a path that was renamed away", () => {
    const staging = new Staging(root);
    staging.apply("a.ts", {
      primary: { kind: "unmodified" },
      secondary: [
        { path: "b.ts", result: { kind: "modified", content: "b\n", renameTo: "gone.ts" } },
      ],
    });
    expect(() => staging.apply("b.ts", modified("B\n"))).toThrow(/renamed away/);
  });

  it("rejects paths that escape the root before staging anything", () => {
    const staging = new Staging(root);
    for (const bad of ["../x.ts", "/etc/passwd", "src/../../x.ts", "C:\\x.ts"]) {
      expect(() => staging.apply("a.ts", modified("x", bad)), bad).toThrow(PathEscapeError);
      expect(() =>
        staging.apply("a.ts", {
          primary: { kind: "unmodified" },
          secondary: [{ path: bad, result: { kind: "modified", content: "x" } }],
        }),
      ).toThrow(PathEscapeError);
    }
    expect(staging.plan()).toEqual({ writes: [], deletes: [] });
  });

  it.skipIf(process.platform === "win32")("rejects symlinks that leave the root", () => {
    const outside = mkdtempSync(join(tmpdir(), "codemod-outside-"));
    try {
      mkdirSync(join(outside, "dir"));
      writeFileSync(join(outside, "secret.ts"), "secret\n");
      symlinkSync(join(outside, "dir"), join(root, "linkdir"));
      symlinkSync(join(outside, "secret.ts"), join(root, "link.ts"));
      symlinkSync(join(outside, "missing.ts"), join(root, "dangling.ts"));
      const staging = new Staging(root);
      expect(() => staging.apply("a.ts", modified("x", "linkdir/new.ts"))).toThrow(/escapes/);
      expect(() => staging.apply("link.ts", modified("x"))).toThrow(/escapes/);
      expect(() => staging.apply("a.ts", modified("x", "dangling.ts"))).toThrow(/broken symlink/);
      expect(() => resolveInsideRoot(root, "linkdir", "x")).toThrow(PathEscapeError);
      expect(resolveInsideRoot(root, "src/new/deep.ts", "x")).toBe(join(root, "src/new/deep.ts"));
      expect(readFileSync(join(outside, "secret.ts"), "utf8")).toBe("secret\n");
    } finally {
      rmSync(outside, { recursive: true, force: true });
    }
  });
});

describe("Staging.commit", () => {
  it("writes every staged file, creates directories, removes renamed sources, and keeps modes", () => {
    const staging = new Staging(root);
    if (process.platform !== "win32") chmodSync(join(root, "a.ts"), 0o755);
    staging.apply("a.ts", modified("A\n"));
    staging.apply("b.ts", modified("B\n", "moved/deep/b.ts"));
    staging.apply("src/c.ts", {
      primary: { kind: "skipped" },
      secondary: [{ path: "src/new.ts", result: { kind: "modified", content: "N\n" } }],
    });
    const report = staging.commit();
    expect(report).toEqual({
      written: ["a.ts", "moved/deep/b.ts", "src/new.ts"],
      deleted: ["b.ts"],
    });
    expect(read("a.ts")).toBe("A\n");
    expect(read("moved/deep/b.ts")).toBe("B\n");
    expect(read("src/new.ts")).toBe("N\n");
    expect(existsSync(join(root, "b.ts"))).toBe(false);
    if (process.platform !== "win32") expect(statSync(join(root, "a.ts")).mode & 0o777).toBe(0o755);
    expect(readdirNames(root).some((name) => name.includes("codemod-tmp"))).toBe(false);
  });

  it.skipIf(process.platform === "win32" || process.getuid?.() === 0)(
    "reports applied and remaining files when a write fails part-way",
    () => {
      write("ro/d.ts", "d\n");
      const staging = new Staging(root);
      staging.apply("a.ts", modified("A\n"));
      staging.apply("ro/d.ts", modified("D\n"));
      staging.apply("b.ts", modified("B\n", "z.ts"));
      chmodSync(join(root, "ro"), 0o555);
      try {
        const error = catchError(() => staging.commit()) as CommitError;
        expect(error).toBeInstanceOf(CommitError);
        expect(error.detail).toEqual({
          phase: "commit",
          applied: ["a.ts"],
          failed: "ro/d.ts",
          remaining: ["ro/d.ts", "z.ts", "b.ts"],
          aborted: false,
        });
        expect(read("a.ts")).toBe("A\n");
        expect(read("ro/d.ts")).toBe("d\n");
        expect(existsSync(join(root, "z.ts"))).toBe(false);
        expect(read("b.ts")).toBe("b\n");
      } finally {
        chmodSync(join(root, "ro"), 0o755);
      }
    },
  );

  it("stops at the first file when the signal has already fired", () => {
    const staging = new Staging(root);
    staging.apply("a.ts", modified("A\n"));
    const controller = new AbortController();
    controller.abort();
    const error = catchError(() => staging.commit(controller.signal)) as CommitError;
    expect(error.detail).toEqual({
      phase: "commit",
      applied: [],
      failed: "a.ts",
      remaining: ["a.ts"],
      aborted: true,
    });
    expect(read("a.ts")).toBe("a\n");
  });
});

function catchError(fn: () => unknown): unknown {
  try {
    fn();
  } catch (error) {
    return error;
  }
  throw new Error("expected a throw");
}

function readdirNames(dir: string): string[] {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  return (require("node:fs") as typeof import("node:fs")).readdirSync(dir);
}

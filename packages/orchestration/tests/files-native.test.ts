/**
 * Differential test of native file selection (`Host.selectFiles` and
 * `Host.fs.readSources`, implemented by `crates/orchestration-walker` and
 * exposed by the QuickJS host) against the TypeScript walker in
 * `src/execution/files.ts`.
 *
 * Both run through the same `selectFiles()` / `readSelectedSources()` calls:
 * once over the Node host (TypeScript walk, `node:fs` reads) and once over
 * the Node host plus the two native capabilities, which run
 * `orchestration-quickjs-host select` (the exact Rust code the QuickJS
 * context calls). The fixture tree is built to be awkward: nested and
 * ancestor ignore files, negations, a parent-directory exclusion that a
 * negation cannot undo, every `.ignore` / `.gitignore` / `.git/info/exclude`
 * precedence, global excludes, CRLF and BOM, a rule set JS cannot compile,
 * case-insensitive and UTF-16 matching (`?` against an astral character),
 * symlinks, dotfiles, newlines in names, files that are not UTF-8, and
 * include/exclude globs on both override sets. A seeded random sweep then
 * draws ignore files from a pool of tricky patterns.
 *
 * Needs the host binary (`cargo build -p orchestration-quickjs-host`); it is
 * skipped without one unless CODEMOD_REQUIRE_RUNTIMES=1.
 */
import { spawnSync } from "node:child_process";
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
import { dirname, join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Host, HostSelectRequest } from "../src/core/host.ts";
import { readSelectedSources, selectFiles, type Selection } from "../src/execution/files.ts";
import { NODE_HOST } from "../src/host/node-host.ts";
import { defaultQuickjsHostPath } from "../src/host/runtimes/quickjs.ts";

const binary = defaultQuickjsHostPath();
const available = existsSync(binary);
if (!available && process.env.CODEMOD_REQUIRE_RUNTIMES === "1") {
  throw new Error(`quickjs host binary not found at ${binary}`);
}

type NativeResult =
  | { files: string[] }
  | { sources: (string | null)[] }
  | { error: { code: string | null; message: string } };

function runNative(job: unknown): NativeResult {
  const result = spawnSync(binary, ["select"], { input: JSON.stringify([job]), encoding: "utf8" });
  if (result.status !== 0) throw new Error(`select failed: ${result.stderr}`);
  return (JSON.parse(result.stdout) as NativeResult[])[0]!;
}

function unwrap<T>(result: NativeResult, key: "files" | "sources"): T {
  if ("error" in result) {
    throw Object.assign(new Error(result.error.message), { code: result.error.code ?? undefined });
  }
  return (result as Record<string, unknown>)[key] as T;
}

let nativeCalls = 0;
const NATIVE_HOST: Host = {
  ...NODE_HOST,
  fs: {
    ...NODE_HOST.fs,
    readSources(root, paths) {
      nativeCalls++;
      return unwrap<(string | null)[]>(runNative({ root, paths }), "sources").map(
        (content) => content ?? undefined,
      );
    },
  },
  selectFiles(request: HostSelectRequest) {
    nativeCalls++;
    return unwrap<string[]>(runNative({ request }), "files");
  },
};

/**
 * File-system errors carry each host's own wording (`EACCES: permission
 * denied, scandir '/x'` from Node, `EACCES: Permission denied (os error
 * 13), scandir '/x'` from Rust); compare code, syscall, and path.
 */
function describeError(error: unknown): string {
  const message = (error as Error).message;
  const io = /^([A-Z]+): .*, (\w+) '(.*)'$/su.exec(message);
  return io ? `${io[1]} ${io[2]} '${io[3]}'` : message;
}

function outcome<T>(run: () => T): { value: T } | { error: string } {
  try {
    return { value: run() };
  } catch (error) {
    return { error: describeError(error) };
  }
}

/**
 * The reference for reads: the Node host with the QuickJS host's
 * `readText(fatal)`, which keeps a leading BOM (`readSources` is that call
 * in a batch). The Node host's `TextDecoder` strips it: a difference between
 * the two hosts that predates native reads.
 */
const keepBom = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const READ_REFERENCE: Host = {
  ...NODE_HOST,
  fs: {
    ...NODE_HOST.fs,
    readText(file, options) {
      if (!options?.fatal) return NODE_HOST.fs.readText(file, options);
      const bytes = readFileSync(file);
      try {
        return keepBom.decode(bytes);
      } catch {
        throw Object.assign(new Error(`'${file}' is not valid UTF-8`), { code: "EILSEQ" });
      }
    },
  },
};

/** Selection and reads, TypeScript vs native; returns the TypeScript side. */
function compare(selection: Omit<Selection, "host">) {
  const before = nativeCalls;
  const typescript = outcome(() => selectFiles({ ...selection, host: NODE_HOST }));
  const native = outcome(() => selectFiles({ ...selection, host: NATIVE_HOST }));
  expect(nativeCalls, "the native path was taken").toBeGreaterThan(before);
  expect(native).toEqual(typescript);
  if ("value" in typescript) {
    const paths = typescript.value;
    expect(outcome(() => readSelectedSources(NATIVE_HOST, selection.targetRoot, paths))).toEqual(
      outcome(() => readSelectedSources(READ_REFERENCE, selection.targetRoot, paths)),
    );
  }
  return typescript;
}

function write(root: string, files: Record<string, string | Buffer>) {
  for (const [relative, content] of Object.entries(files)) {
    const path = join(root, relative);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, content);
  }
}

const ROOT_GITIGNORE = [
  "﻿# a comment after a BOM",
  "*.log",
  "!keep.log",
  "foo/*",
  "!foo/sub/",
  "build/",
  "/anchored.ts",
  "**/gen/**",
  "a?c.ts",
  "ab?c.ts",
  "ab??d.ts",
  "[a-c]x.ts",
  "[!a]y.ts",
  "[z-a]q.ts",
  "\\#hash.ts",
  "\\!bang.ts",
  "spaced.ts   ",
  "escaped\\ space.ts\\ ",
  "**/deep/**/x.ts",
  "abc.*/",
  "ü*.ts",
  "pdir/",
  "keep-me.ts",
  "CaseDir/",
  "*.TMP",
  "**/z.ts",
  "trailing\\",
  "",
  "   ",
].join("\n");

const FILES: Record<string, string | Buffer> = {
  // Ancestor ignore files apply beneath them, relative to their directory.
  "outer/.gitignore": "repo/ancestor-anchored.ts\nancestor-any.ts\n",
  "outer/.ignore": "*.outer-ignore\n",
  "outer/repo/.gitignore": ROOT_GITIGNORE,
  // `.ignore` beats `.gitignore`: `pdir/` stays visited, but its files are
  // still ignored through the `.gitignore` parent rule.
  "outer/repo/.ignore": "!pdir/\n!keep-me.ts\nscratch/\n",
  "outer/repo/.git/HEAD": "ref: refs/heads/main\n",
  "outer/repo/.git/info/exclude": "local-exclude.ts\n",
  "outer/repo/a.ts": "export const a = 1;\n",
  "outer/repo/A2.TS": "upper\n",
  "outer/repo/b.tsx": "b\n",
  "outer/repo/c.mts": "c\n",
  "outer/repo/d.cts": "d\n",
  "outer/repo/e.js": "e\n",
  "outer/repo/f.MJS": "f\n",
  "outer/repo/g.cjs": "g\n",
  "outer/repo/readme.md": "# readme\n",
  "outer/repo/keep.log": "kept\n",
  "outer/repo/x.log": "ignored\n",
  "outer/repo/anchored.ts": "",
  "outer/repo/sub/anchored.ts": "",
  "outer/repo/foo/x.ts": "",
  "outer/repo/foo/sub/y.ts": "",
  "outer/repo/foo/sub/z/w.ts": "",
  "outer/repo/build/out.ts": "",
  "outer/repo/src/build/o.ts": "",
  "outer/repo/gen/g.ts": "",
  "outer/repo/src/gen/deep/g.ts": "",
  "outer/repo/abc.ts": "",
  "outer/repo/a1c.ts": "",
  "outer/repo/a/c.ts": "",
  "outer/repo/abxc.ts": "",
  "outer/repo/ab\u{1F600}c.ts": "",
  "outer/repo/ab\u{1F600}d.ts": "",
  "outer/repo/ax.ts": "",
  "outer/repo/dx.ts": "",
  "outer/repo/ay.ts": "",
  "outer/repo/by.ts": "",
  "outer/repo/!y.ts": "",
  "outer/repo/zq.ts": "",
  "outer/repo/#hash.ts": "",
  "outer/repo/!bang.ts": "",
  "outer/repo/spaced.ts": "",
  "outer/repo/escaped space.ts ": "",
  "outer/repo/deep/1/2/x.ts": "",
  "outer/repo/src/deep/x.ts": "",
  "outer/repo/abc.d/f.ts": "",
  "outer/repo/abc.e": "",
  "outer/repo/über.ts": "",
  "outer/repo/ÜBER2.ts": "",
  "outer/repo/pdir/p.ts": "",
  "outer/repo/keep-me.ts": "",
  "outer/repo/casedir/y.ts": "",
  "outer/repo/foo.tmp": "",
  "outer/repo/local-exclude.ts": "",
  "outer/repo/scratch/s.ts": "",
  "outer/repo/.hidden/h.ts": "",
  "outer/repo/.env": "SECRET=1\n",
  "outer/repo/\u{1F600}.ts": "",
  "outer/repo/\u{1F600}/x.ts": "",
  "outer/repo/\u{FF5E}.ts": "",
  "outer/repo/café.ts": "",
  "outer/repo/new\nline/z.ts": "",
  "outer/repo/sep dir/z.ts": "",
  "outer/repo/plain/z.ts": "",
  "outer/repo/-dash.ts": "",
  "outer/repo/a b.ts": "",
  "outer/repo/x.outer-ignore": "",
  "outer/repo/ancestor-anchored.ts": "",
  "outer/repo/x/ancestor-anchored.ts": "",
  "outer/repo/x/ancestor-any.ts": "",
  "outer/repo/x.gex": "",
  "outer/repo/gabs.ts": "",
  // CRLF and a nested negation.
  "outer/repo/src/nested/.gitignore": "deep.ts\r\n!keep.generated.ts\r\n*.generated.ts\r\n",
  "outer/repo/src/nested/deep.ts": "",
  "outer/repo/src/nested/kept.ts": "",
  "outer/repo/src/nested/x.generated.ts": "",
  "outer/repo/src/nested/keep.generated.ts": "",
  // JS cannot compile `foo[bar/baz`: the whole file is dropped.
  "outer/repo/src/bad/.gitignore": "bad.ts\nfoo[bar/baz\n",
  "outer/repo/src/bad/bad.ts": "",
  // A directory named `.gitignore` is no ignore file.
  "outer/repo/src/dirgi/.gitignore/inner.ts": "",
  "outer/repo/src/dirgi/x.ts": "",
  "outer/repo/src/empty/.gitignore": "",
  "outer/repo/src/empty/e.ts": "",
  "outer/repo/src/a.ts": "export const s = 1;\n",
  "outer/repo/src/keep.ts": "",
  "outer/repo/src/b.tsx": "",
  // A nested repository: its own exclude, parent rules still apply.
  "outer/repo/sub/.git/info/exclude": "local.ts\n",
  "outer/repo/sub/local.ts": "",
  "outer/repo/sub/keep.ts": "",
  "outer/repo/sub/x.log": "",
  "outer/repo/packages/a/.gitignore": "*.skip.ts\n",
  "outer/repo/packages/a/src/m.ts": "export const m = 1;\n",
  "outer/repo/packages/a/src/n.skip.ts": "",
  "outer/repo/packages/a/README.md": "",
  "outer/repo/packages/a/lib/l.js": "",
  "outer/repo/packages/b/z.ts": "",
  // Not UTF-8: selected, but skipped by the reads.
  "outer/repo/bin/latin1.ts": Buffer.from([0x63, 0x61, 0x66, 0xe9, 0x0a]),
  "outer/repo/bin/bom.ts": Buffer.from([0xef, 0xbb, 0xbf, 0x78, 0x0a]),
  "outer/repo/bin/ok.ts": "ok\n",
};

let base: string;
let repo: string;
let globalExcludes: string;

beforeAll(() => {
  base = realpathSync(mkdtempSync(join(tmpdir(), "codemod-files-native-")));
  repo = join(base, "outer/repo");
  write(base, FILES);
  globalExcludes = join(base, "global-excludes");
  writeFileSync(
    globalExcludes,
    `*.gex\nrepo/global-anchored.ts\n${join(base, "outer/repo/gabs.ts").slice(1)}\n`,
  );
  symlinkSync(join(repo, "a.ts"), join(repo, "link.ts"));
  symlinkSync(join(repo, "src"), join(repo, "linkdir"));
  symlinkSync(join(repo, "missing"), join(repo, "broken.ts"));
  // A name that is not UTF-8, where the file system allows one (APFS does not).
  try {
    writeFileSync(Buffer.from(`${repo}/raw-\xff.ts`, "latin1"), "");
  } catch {
    // EILSEQ: the case does not exist on this file system.
  }
});
afterAll(() => rmSync(base, { recursive: true, force: true }));

const run = available ? describe : describe.skip;

run("native selection matches the TypeScript walker", () => {
  const cases: [string, () => Omit<Selection, "host">][] = [
    ["ignore files only", () => sel({ language: "none" })],
    ["language defaults (typescript)", () => sel({ language: "typescript" })],
    ["language defaults (javascript)", () => sel({ language: "javascript" })],
    ["global excludes", () => sel({ language: "none", globalExcludes })],
    [
      "definition include with braces, excludes",
      () =>
        sel({
          definition: { include: ["**/*.{ts,TS,log}"], exclude: ["**/gen/**", "src/nested"] },
        }),
    ],
    [
      "invocation include/exclude with negations",
      () => sel({ invocation: { include: ["src/**", "!src/bad/**"], exclude: ["!*.tsx"] } }),
    ],
    [
      "definition include and invocation exclude",
      () => sel({ definition: { include: ["*.ts"] }, invocation: { exclude: ["foo"] } }),
    ],
    ["include everything overrides ignore files", () => sel({ definition: { include: ["**/*"] } })],
    [
      "include with ? and ranges",
      () => sel({ definition: { include: ["ab?c.ts", "[a-c]x.ts", "**/x.ts", "?.ts"] } }),
    ],
    ["exclude only", () => sel({ definition: { exclude: ["*.ts", "/src"] } })],
    ["nested braces", () => sel({ definition: { include: ["{src,packages/{a,b}}/**/*.ts"] } })],
    [
      "target root beneath the repository",
      () =>
        sel({
          targetRoot: join(repo, "packages/a"),
          definition: { include: ["packages/a/**/*.ts", "packages/b/**"] },
          invocation: { include: ["src/*.ts", "lib/**"] },
        }),
    ],
    [
      "target root with language defaults and global excludes",
      () => sel({ targetRoot: join(repo, "src"), language: "typescript", globalExcludes }),
    ],
  ];

  function sel(partial: Partial<Omit<Selection, "host">>): Omit<Selection, "host"> {
    return {
      cwd: repo,
      targetRoot: repo,
      language: "none",
      definition: {},
      invocation: {},
      globalExcludes: null,
      ...partial,
    };
  }

  it.each(cases)("%s", (_name, selection) => {
    const result = compare(selection());
    expect("value" in result && result.value.length > 0, "selects something").toBe(true);
  });

  it("pins the semantics the native side must reproduce", () => {
    const result = compare(sel({ language: "none" }));
    if (!("value" in result)) throw new Error(result.error);
    const files = result.value;
    // A parent directory excluded by `.gitignore` wins over `.ignore`'s `!pdir/`.
    expect(files).not.toContain("pdir/p.ts");
    // `.ignore` beats `.gitignore` for the same file.
    expect(files).toContain("keep-me.ts");
    // Case-insensitive matching (`CaseDir/`, `*.TMP`, `ü*.ts` vs `ÜBER2.ts`).
    expect(files).not.toContain("casedir/y.ts");
    expect(files).not.toContain("foo.tmp");
    expect(files).not.toContain("ÜBER2.ts");
    // `?` is one UTF-16 unit: `ab?c.ts` misses the emoji, `ab??d.ts` hits it.
    expect(files).toContain("ab\u{1F600}c.ts");
    expect(files).not.toContain("ab\u{1F600}d.ts");
    // `.` stops at line terminators: `**/z.ts` misses `new\nline/z.ts`.
    expect(files).toContain("new\nline/z.ts");
    expect(files).not.toContain("plain/z.ts");
    // The rule set JS cannot compile is dropped whole; CRLF lines work.
    expect(files).toContain("src/bad/bad.ts");
    expect(files).not.toContain("src/nested/deep.ts");
    // Symlinks are never selected; ancestors' ignore files apply.
    expect(files).not.toContain("link.ts");
    expect(files).not.toContain("x.outer-ignore");
    expect(files).not.toContain("ancestor-anchored.ts");
    expect(files).toContain("x/ancestor-anchored.ts");
    // UTF-16 code unit order: U+1F600 (a surrogate pair) sorts before U+FF5E.
    expect(files.indexOf("\u{1F600}.ts")).toBeLessThan(files.indexOf("\u{FF5E}.ts"));
  });

  it("reports a path the ignore package refuses exactly", () => {
    const dir = join(base, "dots");
    write(dir, { ".gitignore": "", "...": "", "ok.ts": "" });
    const result = compare(sel({ cwd: dir, targetRoot: dir }));
    expect(result).toEqual({
      error: 'path should be a `path.relative()`d string, but got "..."',
    });
  });

  it.skipIf(process.getuid?.() === 0)("reports an unreadable directory exactly", () => {
    const dir = join(base, "locked");
    write(dir, { "a.ts": "", "closed/b.ts": "" });
    chmodSync(join(dir, "closed"), 0o000);
    try {
      const result = compare(sel({ cwd: dir, targetRoot: dir }));
      expect(result).toEqual({ error: `EACCES scandir '${join(dir, "closed")}'` });
    } finally {
      chmodSync(join(dir, "closed"), 0o755);
    }
  });

  it.skipIf(process.getuid?.() === 0)(
    "reports the first of several failures in the sequential walk's order",
    () => {
      // Failures in sibling subtrees (walked on parallel threads natively):
      // an unreadable directory, a refused path, another unreadable one.
      const dir = join(base, "many-failures");
      const tree: Record<string, string> = { ".gitignore": "" };
      for (const name of ["a", "b", "c", "d", "e", "f", "g", "h"]) {
        tree[`${name}/x/ok.ts`] = "";
        tree[`${name}/y/ok.ts`] = "";
      }
      // `...` is refused where it is tested relative to its own directory.
      tree["d/y/..."] = "";
      tree["d/y/.gitignore"] = "";
      write(dir, tree);
      const locked = [join(dir, "c/x"), join(dir, "f/y")];
      for (const path of locked) chmodSync(path, 0o000);
      try {
        const result = compare(sel({ cwd: dir, targetRoot: dir }));
        expect("error" in result).toBe(true);
      } finally {
        for (const path of locked) chmodSync(path, 0o755);
      }
    },
  );

  it("reports an unreadable selected file exactly", () => {
    const dir = join(base, "unreadable");
    write(dir, { "a.ts": "a\n", "b.ts": "b\n" });
    chmodSync(join(dir, "b.ts"), 0o000);
    try {
      if (process.getuid?.() === 0) return;
      const read = (host: Host) => outcome(() => readSelectedSources(host, dir, ["a.ts", "b.ts"]));
      expect(read(NATIVE_HOST)).toEqual(read(READ_REFERENCE));
      expect(read(READ_REFERENCE)).toEqual({ error: `EACCES open '${join(dir, "b.ts")}'` });
    } finally {
      chmodSync(join(dir, "b.ts"), 0o644);
    }
  });

  it("agrees on random ignore files drawn from tricky patterns", () => {
    const pool = [
      "*.ts",
      "!*.ts",
      "*.TS",
      "src",
      "src/",
      "/src",
      "!src/",
      "src/*",
      "!src/a.ts",
      "src/**",
      "**/src/**",
      "**/*.tsx",
      "a.ts",
      "/a.ts",
      "!/a.ts",
      "**/a.ts",
      "foo/*",
      "!foo/sub/",
      "foo/sub/**",
      "*.log",
      "!keep.log",
      "**/deep/**",
      "deep/**/x.ts",
      "[a-c]*",
      "[!a]*.ts",
      "?.ts",
      "??",
      "ab?c.ts",
      "ab??c.ts",
      "*",
      "!*/",
      "**",
      ".hidden",
      ".*",
      "!.gitignore",
      "\\!bang.ts",
      "\\#hash.ts",
      "#comment",
      "spaced.ts   ",
      "escaped\\ space.ts\\ ",
      "ü*",
      "\u{1F600}*",
      "packages/*/src",
      "packages/**/lib/",
      "*.{ts,js}",
      "a/**/",
      "z-a].ts",
      "x}y",
      "[z-a]",
      "trailing\\",
      "nested/../a.ts",
      "./a.ts",
    ];
    let seed = 0x2f6e2b1;
    const random = () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed / 0x80000000;
    };
    const pick = () =>
      Array.from({ length: 1 + Math.floor(random() * 6) }, () => {
        const pattern = pool[Math.floor(random() * pool.length)]!;
        return random() < 0.15 ? `!${pattern.replace(/^!/u, "")}` : pattern;
      }).join(random() < 0.2 ? "\r\n" : "\n");
    const places = [".gitignore", ".ignore", "src/.gitignore", "foo/.gitignore", "sub/.gitignore"];
    const sweep = join(base, "sweep");
    // CODEMOD_WALKER_SWEEP_ROUNDS raises it for a longer search.
    const rounds = Number(process.env.CODEMOD_WALKER_SWEEP_ROUNDS ?? 40);
    for (let round = 0; round < rounds; round++) {
      rmSync(sweep, { recursive: true, force: true });
      const tree: Record<string, string> = {};
      for (const [path, content] of Object.entries(FILES)) {
        if (path.startsWith("outer/repo/") && !path.endsWith("ignore")) {
          tree[path.slice("outer/repo/".length)] = typeof content === "string" ? content : "";
        }
      }
      for (const place of places) if (random() < 0.6) tree[place] = pick();
      write(sweep, tree);
      const include = random() < 0.3 ? [pick().split(/\r?\n/u)[0]!] : undefined;
      compare(sel({ cwd: sweep, targetRoot: sweep, definition: include ? { include } : {} }));
    }
  });
});

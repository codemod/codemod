/**
 * File selection: the walker contract shared with the workflow engine
 * (`fixtures/walker/cases.json`, also run by
 * `crates/execution-bridge/tests/contracts.rs`), definition/target
 * intersection, language defaults, and global excludes discovery.
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  comparePaths,
  discoverGlobalExcludesPath,
  languageExtensions,
  selectFiles,
  type Selection,
} from "../src/index.ts";

interface Contract {
  files: Record<string, string>;
  symlinks?: Record<string, string>;
  cases: { name: string; include?: string[]; exclude?: string[]; expected: string[] }[];
}

const contract = JSON.parse(
  readFileSync(join(import.meta.dirname, "../fixtures/walker/cases.json"), "utf8"),
) as Contract;

function materialize(files: Record<string, string>, symlinks: Record<string, string> = {}) {
  const dir = mkdtempSync(join(tmpdir(), "codemod-files-"));
  for (const [relative, content] of Object.entries(files)) {
    const path = join(dir, relative);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, content);
  }
  if (process.platform !== "win32") {
    for (const [link, target] of Object.entries(symlinks)) {
      symlinkSync(join(dir, target), join(dir, link));
    }
  }
  return dir;
}

describe("walker parity with the workflow engine", () => {
  let dir: string;
  beforeAll(() => {
    dir = materialize(contract.files, contract.symlinks);
  });
  afterAll(() => rmSync(dir, { recursive: true, force: true }));

  it.each(contract.cases.map((c) => [c.name, c] as const))("%s", (_name, c) => {
    // A language without extensions and no include leaves only the case's
    // own globs as overrides, exactly like the engine side of the contract.
    const selection: Selection = {
      cwd: dir,
      targetRoot: dir,
      language: "none",
      definition: {},
      invocation: { include: c.include, exclude: c.exclude },
      globalExcludes: null,
    };
    expect(selectFiles(selection)).toEqual(c.expected);
  });

  it("sorts component-wise like Rust PathBuf ordering", () => {
    expect(["a-b/c.ts", "a/b.ts", "a", "a/b", "b"].sort(comparePaths)).toEqual([
      "a",
      "a/b",
      "a/b.ts",
      "a-b/c.ts",
      "b",
    ]);
  });
});

describe("selectFiles", () => {
  let repo: string;
  beforeAll(() => {
    repo = materialize({
      "app/src/a.ts": "",
      "app/src/b.tsx": "",
      "app/src/c.generated.ts": "",
      "app/src/notes.md": "",
      "app/other/d.ts": "",
      "app/.gitignore": "dist/\n",
      "app/dist/e.ts": "",
      "lib/f.ts": "",
    });
  });
  afterAll(() => rmSync(repo, { recursive: true, force: true }));

  const select = (overrides: Partial<Selection>) =>
    selectFiles({
      cwd: repo,
      targetRoot: join(repo, "app"),
      language: "none",
      definition: {},
      invocation: {},
      globalExcludes: null,
      ...overrides,
    });

  it.each<[string, Partial<Selection>, string[]]>([
    [
      "the language's extensions when the definition has no include",
      { language: "tsx" },
      ["other/d.ts", "src/a.ts", "src/b.tsx", "src/c.generated.ts"],
    ],
    [
      "everything the target names for a language without extensions",
      { invocation: { include: ["src/**"] } },
      ["src/a.ts", "src/b.tsx", "src/c.generated.ts", "src/notes.md"],
    ],
    [
      "repository-relative definition globs intersected with target-relative ones",
      {
        language: "typescript",
        definition: { include: ["app/**/*.{ts,tsx}"], exclude: ["**/*.d.ts"] },
        invocation: { include: ["src/**"], exclude: ["**/*.generated.ts"] },
      },
      ["src/a.ts", "src/b.tsx"],
    ],
    [
      "nothing for a definition include naming another repository area",
      { language: "typescript", definition: { include: ["lib/**/*.ts"] } },
      [],
    ],
  ])("selects %s", (_name, overrides, expected) => {
    expect(select(overrides)).toEqual(expected);
  });

  it("honors an explicit global excludes file unless an include glob whitelists the file", () => {
    const global = join(repo, "global-ignore");
    writeFileSync(global, "*.md\n");
    expect(select({ globalExcludes: global })).toEqual([
      ".gitignore",
      "other/d.ts",
      "src/a.ts",
      "src/b.tsx",
      "src/c.generated.ts",
    ]);
    expect(select({ globalExcludes: global, invocation: { include: ["src/**"] } })).toEqual([
      "src/a.ts",
      "src/b.tsx",
      "src/c.generated.ts",
      "src/notes.md",
    ]);
  });

  it("reads the language table pinned to the engine", () => {
    expect(languageExtensions("typescript")).toContain(".ts");
    expect(languageExtensions("typescript")).not.toContain(".tsx");
    expect(languageExtensions("none")).toEqual([]);
  });
});

describe("discoverGlobalExcludesPath", () => {
  it("follows ~/.gitconfig, then XDG config, then the XDG default like the ignore crate", () => {
    const home = mkdtempSync(join(tmpdir(), "codemod-home-"));
    try {
      const xdg = join(home, "xdg");
      mkdirSync(join(xdg, "git"), { recursive: true });
      expect(discoverGlobalExcludesPath({ HOME: home, XDG_CONFIG_HOME: xdg })).toBe(
        join(xdg, "git", "ignore"),
      );
      expect(discoverGlobalExcludesPath({ HOME: home, XDG_CONFIG_HOME: "" })).toBe(
        join(home, ".config", "git", "ignore"),
      );
      writeFileSync(join(xdg, "git", "config"), "[core]\n\texcludesfile = ~/from-xdg\n");
      expect(discoverGlobalExcludesPath({ HOME: home, XDG_CONFIG_HOME: xdg })).toBe(
        join(home, "from-xdg"),
      );
      writeFileSync(join(home, ".gitconfig"), '[core]\n  ExcludesFile = "/abs/global-ignore"\n');
      expect(discoverGlobalExcludesPath({ HOME: home, XDG_CONFIG_HOME: xdg })).toBe(
        "/abs/global-ignore",
      );
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  });
});

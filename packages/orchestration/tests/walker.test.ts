/**
 * The TypeScript side of the walker contract shared with the workflow
 * engine (`fixtures/walker/cases.json`, also run by
 * `crates/execution-bridge/tests/walker_parity.rs`), plus file selection
 * (definition applicability intersected with the invocation target) and the
 * global git excludes discovery.
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  OverrideMatcher,
  comparePaths,
  discoverGlobalExcludesPath,
  selectFiles,
  walkFiles,
} from "../src/index.ts";

interface Contract {
  files: Record<string, string>;
  symlinks?: Record<string, string>;
  cases: { name: string; include?: string[]; exclude?: string[]; expected: string[] }[];
}

const contract = JSON.parse(
  readFileSync(join(import.meta.dirname, "../fixtures/walker/cases.json"), "utf8"),
) as Contract;

function materialize(
  dir: string,
  files: Record<string, string>,
  symlinks: Record<string, string> = {},
) {
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
}

describe("walker parity with the workflow engine", () => {
  let dir: string;
  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), "codemod-walker-"));
    materialize(dir, contract.files, contract.symlinks);
  });
  afterAll(() => rmSync(dir, { recursive: true, force: true }));

  it.each(contract.cases.map((c) => [c.name, c] as const))("%s", (_name, c) => {
    const overrides =
      c.include === undefined && c.exclude === undefined
        ? []
        : [{ root: dir, matcher: new OverrideMatcher({ include: c.include, exclude: c.exclude }) }];
    expect(walkFiles(dir, { overrides })).toEqual(c.expected);
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
    repo = mkdtempSync(join(tmpdir(), "codemod-select-"));
    materialize(repo, {
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

  it("uses the language extensions when the definition has no include", () => {
    expect(
      selectFiles({
        cwd: repo,
        targetRoot: join(repo, "app"),
        definition: {},
        invocation: {},
        extensions: [".ts", ".tsx"],
        globalExcludes: null,
      }),
    ).toEqual(["other/d.ts", "src/a.ts", "src/b.tsx", "src/c.generated.ts"]);
  });

  it("selects nothing extension-wise for a language without extensions and no include", () => {
    expect(
      selectFiles({
        cwd: repo,
        targetRoot: join(repo, "app"),
        definition: {},
        invocation: { include: ["src/**"] },
        extensions: [],
        globalExcludes: null,
      }),
    ).toEqual(["src/a.ts", "src/b.tsx", "src/c.generated.ts", "src/notes.md"]);
  });

  it("intersects repository-relative definition globs with target-relative invocation globs", () => {
    expect(
      selectFiles({
        cwd: repo,
        targetRoot: join(repo, "app"),
        definition: { include: ["app/**/*.ts"], exclude: ["**/*.d.ts"] },
        invocation: { include: ["src/**"], exclude: ["**/*.generated.ts"] },
        extensions: [".ts"],
        globalExcludes: null,
      }),
    ).toEqual(["src/a.ts"]);
    // A definition include that names another repository area selects nothing here.
    expect(
      selectFiles({
        cwd: repo,
        targetRoot: join(repo, "app"),
        definition: { include: ["lib/**/*.ts"] },
        invocation: {},
        extensions: [".ts"],
        globalExcludes: null,
      }),
    ).toEqual([]);
  });

  it("reports invalid globs", () => {
    expect(() =>
      selectFiles({
        cwd: repo,
        targetRoot: join(repo, "app"),
        definition: { include: ["["] },
        invocation: {},
        extensions: [],
        globalExcludes: null,
      }),
    ).toThrow(/invalid glob/);
  });

  it("honors an explicit global excludes file unless an include glob whitelists the file", () => {
    const global = join(repo, "global-ignore");
    writeFileSync(global, "*.md\n");
    const select = (invocation: { include?: string[] }) =>
      selectFiles({
        cwd: repo,
        targetRoot: join(repo, "app"),
        definition: {},
        invocation,
        extensions: [],
        globalExcludes: global,
      });
    expect(select({})).toEqual([
      ".gitignore",
      "other/d.ts",
      "src/a.ts",
      "src/b.tsx",
      "src/c.generated.ts",
    ]);
    // Overrides have the highest precedence in the engine's walker.
    expect(select({ include: ["src/**"] })).toEqual([
      "src/a.ts",
      "src/b.tsx",
      "src/c.generated.ts",
      "src/notes.md",
    ]);
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

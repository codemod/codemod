/**
 * Glob and gitignore semantics ported from the Rust `ignore` / `globset`
 * crates. The end-to-end contract with the engine's walker is
 * `tests/walker.test.ts`; these pin the individual rules.
 */
import { describe, expect, it } from "vitest";
import {
  GlobError,
  GitignoreMatcher,
  OverrideMatcher,
  globToRegex,
  parseGitignore,
  parseGitignoreLine,
} from "../src/index.ts";

describe("globToRegex (globset, literal separator, backslash escape)", () => {
  it.each([
    ["*.ts", "a.ts", true],
    ["*.ts", "src/a.ts", false],
    ["**/*.ts", "a.ts", true],
    ["**/*.ts", "src/deep/a.ts", true],
    ["src/**", "src/a.ts", true],
    ["src/**", "src/deep/a.ts", true],
    ["src/**", "src", false],
    ["src/**/*.ts", "src/a.ts", true],
    ["src/**/*.ts", "src/x/y/a.ts", true],
    ["src/**/*.ts", "srcx/a.ts", false],
    ["a/**/b", "a/b", true],
    ["a/**/b", "a/x/b", true],
    ["a/**/b", "a/x/y/b", true],
    ["a**b", "axxb", true],
    ["a**b", "a/b", false],
    ["?.ts", "a.ts", true],
    ["?.ts", "ab.ts", false],
    ["?.ts", "/.ts", false],
    ["[ab].ts", "a.ts", true],
    ["[ab].ts", "c.ts", false],
    ["[!ab].ts", "c.ts", true],
    ["[a-c].ts", "b.ts", true],
    ["[]a].ts", "].ts", true],
    ["*.{ts,tsx}", "a.tsx", true],
    ["*.{ts,tsx}", "a.js", false],
    ["**/*.{ts,md}", "docs/x.md", true],
    ["\\*.ts", "*.ts", true],
    ["\\*.ts", "a.ts", false],
    ["**", "anything/at/all", true],
    ["a.ts", "a.ts", true],
    ["a.ts", "b/a.ts", false],
  ])("%s matches %s -> %s", (glob, path, expected) => {
    expect(globToRegex(glob).test(path)).toBe(expected);
  });

  it("rejects unclosed classes and nested or unclosed alternates", () => {
    expect(() => globToRegex("[abc")).toThrow(GlobError);
    expect(() => globToRegex("{a,{b,c}}")).toThrow(GlobError);
    expect(() => globToRegex("{a,b")).toThrow(GlobError);
    expect(() => globToRegex("[")).toThrow(GlobError);
  });
});

describe("parseGitignoreLine", () => {
  it("handles comments, blanks, negation, anchoring, directory-only, and escapes", () => {
    expect(parseGitignoreLine("# comment")).toBeNull();
    expect(parseGitignoreLine("   ")).toBeNull();
    expect(parseGitignoreLine("")).toBeNull();

    const basename = parseGitignoreLine("build")!;
    expect(basename.regex.test("build")).toBe(true);
    expect(basename.regex.test("a/b/build")).toBe(true);
    expect(basename.onlyDir).toBe(false);

    const anchored = parseGitignoreLine("/build")!;
    expect(anchored.regex.test("build")).toBe(true);
    expect(anchored.regex.test("a/build")).toBe(false);

    const withSlash = parseGitignoreLine("src/gen")!;
    expect(withSlash.regex.test("src/gen")).toBe(true);
    expect(withSlash.regex.test("x/src/gen")).toBe(false);

    const dirOnly = parseGitignoreLine("ignored/")!;
    expect(dirOnly.onlyDir).toBe(true);
    expect(dirOnly.regex.test("ignored")).toBe(true);
    expect(dirOnly.regex.test("a/ignored")).toBe(true);

    const whitelist = parseGitignoreLine("!keep.ts")!;
    expect(whitelist.whitelist).toBe(true);

    const escapedBang = parseGitignoreLine("\\!literal")!;
    expect(escapedBang.whitelist).toBe(false);
    expect(escapedBang.regex.test("!literal")).toBe(true);

    const trailingSpaces = parseGitignoreLine("a.ts   ")!;
    expect(trailingSpaces.regex.test("a.ts")).toBe(true);
    const escapedSpace = parseGitignoreLine("a\\ ")!;
    expect(escapedSpace.regex.test("a ")).toBe(true);

    const contents = parseGitignoreLine("dist/**")!;
    expect(contents.regex.test("dist")).toBe(false);
    expect(contents.regex.test("dist/x")).toBe(true);
    expect(contents.regex.test("dist/x/y")).toBe(true);
  });

  it("skips unparseable lines when reading a whole file", () => {
    const globs = parseGitignore("good.ts\n[broken\n!keep.ts\n");
    expect(globs.map((glob) => glob.original)).toEqual(["good.ts", "!keep.ts"]);
  });
});

describe("GitignoreMatcher", () => {
  const matcher = new GitignoreMatcher(parseGitignore("*.log\n!keep.log\nbuild/\n"));

  it("lets the last matching pattern win and applies dir-only patterns to directories", () => {
    expect(matcher.match("x.log", false)).toBe("ignore");
    expect(matcher.match("keep.log", false)).toBe("whitelist");
    expect(matcher.match("build", true)).toBe("ignore");
    expect(matcher.match("build", false)).toBe("none");
    expect(matcher.match("./x.log", false)).toBe("ignore");
    expect(matcher.match("other.ts", false)).toBe("none");
  });
});

describe("OverrideMatcher", () => {
  it("whitelists includes, ignores excludes, and ignores unmatched files only when includes exist", () => {
    const both = new OverrideMatcher({ include: ["**/*.ts"], exclude: ["**/*.generated.ts"] });
    expect(both.match("a.ts", false)).toBe("whitelist");
    expect(both.match("a.generated.ts", false)).toBe("ignore");
    expect(both.match("a.md", false)).toBe("ignore");
    expect(both.match("src", true)).toBe("none");

    const excludeOnly = new OverrideMatcher({ exclude: ["docs/**"] });
    expect(excludeOnly.match("a.md", false)).toBe("none");
    expect(excludeOnly.match("docs/a.md", false)).toBe("ignore");
    expect(excludeOnly.whitelists).toBe(0);

    const dirInclude = new OverrideMatcher({ include: ["src/**"] });
    expect(dirInclude.match("src/build", true)).toBe("whitelist");
    expect(dirInclude.match("src", true)).toBe("none");
    expect(dirInclude.match("other", true)).toBe("none");
    expect(dirInclude.match("other/a.ts", false)).toBe("ignore");

    const leadingBang = new OverrideMatcher({ exclude: ["!gen"] });
    expect(leadingBang.match("gen", true)).toBe("ignore");
    expect(new OverrideMatcher({}).empty).toBe(true);
  });

  it("rejects invalid patterns", () => {
    expect(() => new OverrideMatcher({ include: ["["] })).toThrow(GlobError);
    expect(() => new OverrideMatcher({ exclude: ["{a,{b}}"] })).toThrow(GlobError);
    expect(() => new OverrideMatcher({ include: ["# no"] })).toThrow(GlobError);
  });
});

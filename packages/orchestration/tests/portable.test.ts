/**
 * The host seam, statically and behaviorally:
 *
 * - everything reachable from `src/portable.ts` (authoring, `run()`, replay,
 *   history, scheduling, and the JSSG + shell
 *   execution path) imports no `node:*` module, no Node-only package, and uses
 *   no Node global, and no other `src` file imports `node:*` either;
 * - `core/posix.ts` matches `node:path.posix`.
 */
import { readdirSync, readFileSync } from "node:fs";
import { posix as nodePosix, dirname, join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { createPosixPath } from "../src/portable.ts";

const src = resolve(import.meta.dirname, "../src");
const rel = (file: string) => relative(src, file).split("\\").join("/");

/** Pure-JavaScript packages the portable closure may bundle. */
const PORTABLE_PACKAGES = new Set(["ignore"]);

/** Globals a QuickJS host does not provide (the host seam replaces them). */
const NODE_GLOBALS: [RegExp, string][] = [
  [/\bprocess\s*\./u, "process"],
  [/\bBuffer\b/u, "Buffer"],
  [/\brequire\s*\(/u, "require"],
  [/\b__dirname\b|\b__filename\b/u, "__dirname"],
  [/import\.meta\.(?:dirname|filename|url|resolve)/u, "import.meta"],
  // Global calls only: `host.setTimeout(...)` and the `Host` interface's own
  // method declarations (at the start of a line) are the sanctioned path.
  [
    /(?<!^[ \t]*|[.\w])(?:setTimeout|setInterval|setImmediate|clearTimeout|clearInterval)\s*\(/mu,
    "timers",
  ],
  [/\bTextDecoder\b|\bTextEncoder\b/u, "TextDecoder"],
  [/\bAsyncLocalStorage\b/u, "AsyncLocalStorage"],
];

const stripComments = (text: string) =>
  text.replace(/\/\*[\s\S]*?\*\//gu, "").replace(/(^|[^:"'`\\])\/\/.*$/gmu, "$1");

/** Value imports and re-exports (type-only ones are erased and do not load). */
function valueImports(text: string): string[] {
  const code = stripComments(text);
  const found: string[] = [];
  for (const match of code.matchAll(
    /(?:^|[\n;])\s*(import|export)\s+(type\s+)?(?:[^;"']*?\sfrom\s+)?"([^"]+)"/gu,
  )) {
    if (match[2] === undefined) found.push(match[3]!);
  }
  for (const match of code.matchAll(/\bimport\(\s*"([^"]+)"\s*\)/gu)) found.push(match[1]!);
  return found;
}

function portableClosure(entry: string): Map<string, string[]> {
  const seen = new Map<string, string[]>();
  const visit = (file: string) => {
    if (seen.has(file) || !file.endsWith(".ts")) return;
    const specifiers = valueImports(readFileSync(file, "utf8"));
    seen.set(file, specifiers);
    for (const specifier of specifiers) {
      if (specifier.startsWith(".")) visit(resolve(dirname(file), specifier));
    }
  };
  visit(entry);
  return seen;
}

describe("the portable closure", () => {
  const closure = portableClosure(join(src, "portable.ts"));

  it("covers the control plane and the JSSG + shell execution path", () => {
    const files = [...closure.keys()].map(rel);
    for (const required of [
      "authoring/composition.ts",
      "authoring/context.ts",
      "core/history.ts",
      "core/paths.ts",
      "execution/files.ts",
      "execution/jssg.ts",
      "execution/scheduler.ts",
      "execution/shell.ts",
      "runtime/gate.ts",
      "runtime/run.ts",
    ]) {
      expect(files).toContain(required);
    }
  });

  it("imports no node:* module and no Node-only package", () => {
    const violations: string[] = [];
    for (const [file, specifiers] of closure) {
      for (const specifier of specifiers) {
        if (specifier.startsWith(".")) continue;
        if (!PORTABLE_PACKAGES.has(specifier)) violations.push(`${rel(file)} -> ${specifier}`);
      }
    }
    expect(violations).toEqual([]);
  });

  it("uses no Node global", () => {
    const violations: string[] = [];
    for (const file of closure.keys()) {
      const code = stripComments(readFileSync(file, "utf8"));
      for (const [pattern, name] of NODE_GLOBALS) {
        if (pattern.test(code)) violations.push(`${rel(file)} uses ${name}`);
      }
    }
    expect(violations).toEqual([]);
  });

  it("leaves no src file importing node:*", () => {
    const files = readdirSync(src, { recursive: true, encoding: "utf8" })
      .filter((name) => name.endsWith(".ts"))
      .map((name) => join(src, name));
    const violations = files
      .filter((file) =>
        valueImports(readFileSync(file, "utf8")).some((specifier) => specifier.startsWith("node:")),
      )
      .map(rel);
    expect(violations).toEqual([]);
  });

  it("recognizes value imports and ignores type-only ones (scanner self-check)", () => {
    expect(
      valueImports(
        [
          'import type { A } from "node:fs";',
          'export type { B } from "node:os";',
          'import { type C, d } from "node:path";',
          'import "./side-effect.ts";',
          'import e from "ignore";',
          'export { f } from "./f.ts";',
          '// import { g } from "node:crypto";',
          'const h = await import("node:url");',
          'import {\n  i,\n  j,\n} from "node:child_process";',
        ].join("\n"),
      ),
    ).toEqual([
      "node:path",
      "./side-effect.ts",
      "ignore",
      "./f.ts",
      "node:child_process",
      "node:url",
    ]);
  });
});

describe("the QuickJS bundle entry (host/quickjs/entry.ts) closure", () => {
  // Everything the quickjs bundle evaluates inside QuickJS: the entry plus
  // the portable control plane it imports.
  const closure = portableClosure(join(src, "host", "quickjs", "entry.ts"));
  const inHost = (file: string) => rel(file).startsWith("host/");

  it("reaches only host/quickjs/* and portable modules", () => {
    const files = [...closure.keys()];
    expect(
      files
        .filter(inHost)
        .map(rel)
        .filter((f) => !f.startsWith("host/quickjs/")),
    ).toEqual([]);
    expect(files.map(rel)).toContain("execution/jssg.ts");
  });

  it("imports no node:* module or Node-only package and uses no Node global", () => {
    const violations: string[] = [];
    for (const [file, specifiers] of closure) {
      for (const specifier of specifiers) {
        if (!specifier.startsWith(".") && !PORTABLE_PACKAGES.has(specifier)) {
          violations.push(`${rel(file)} -> ${specifier}`);
        }
      }
      const code = stripComments(readFileSync(file, "utf8"));
      for (const [pattern, name] of NODE_GLOBALS) {
        if (pattern.test(code)) violations.push(`${rel(file)} uses ${name}`);
      }
    }
    expect(violations).toEqual([]);
  });
});

describe("core/posix.ts", () => {
  const cwd = "/work/dir";
  const ours = createPosixPath(() => cwd);
  const segments = [
    "",
    ".",
    "..",
    "a",
    "b",
    "..a",
    "a..",
    ".hidden",
    "/",
    "//",
    "c.d",
    "a/b",
    "../x",
  ];
  const inputs: string[] = [];
  for (const first of segments) {
    for (const second of segments) {
      for (const third of ["", "/", "e", ".."]) inputs.push(first + "/" + second + third);
      inputs.push(first + second);
    }
  }
  inputs.push("", "/", "//", "///a//b/", "./", "../", "a/./b/../../..", "/a/b/c/../../..");

  it("matches node:path.posix on normalize, dirname, basename, isAbsolute, join", () => {
    for (const input of inputs) {
      expect(ours.normalize(input), input).toBe(nodePosix.normalize(input));
      expect(ours.dirname(input), input).toBe(nodePosix.dirname(input));
      expect(ours.basename(input), input).toBe(nodePosix.basename(input));
      expect(ours.isAbsolute(input), input).toBe(nodePosix.isAbsolute(input));
      expect(ours.join(input, "x", input), input).toBe(nodePosix.join(input, "x", input));
    }
  });

  it("matches node:path.posix on resolve and relative against the given cwd", () => {
    const nodeResolve = (...paths: string[]) => nodePosix.resolve(cwd, ...paths);
    for (const input of inputs) {
      expect(ours.resolve(input), input).toBe(nodeResolve(input));
      expect(ours.resolve("/root", input), input).toBe(nodeResolve("/root", input));
      for (const other of ["/work", "/work/dir/a", "x/y", "/", "../z"]) {
        expect(ours.relative(input, other), `${input} -> ${other}`).toBe(
          nodePosix.relative(nodeResolve(input), nodeResolve(other)),
        );
      }
    }
  });
});

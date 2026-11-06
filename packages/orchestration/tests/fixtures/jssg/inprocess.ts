/**
 * Extra transforms for the bridge-vs-node parity tests
 * (`tests/node-runtime.e2e.test.ts`): the rest of the `codemod:ast-grep`
 * surface a transform reaches (`jssgTransform`, `parse`, `parseFile` +
 * `write()`, `kind`, options) and the result rules. Only ever built.
 */
import * as astGrep from "codemod:ast-grep";
import { jssgTransform, parse } from "codemod:ast-grep";
import { jssg } from "../../../src/index.ts";
import { nextCall } from "./counter.ts";
import { migrateText, posixPath } from "./helpers.ts";

// `kind` and `parseFile` are exported by the runtime module but missing from
// `@codemod.com/jssg-types`; transforms reach them through the namespace (a
// module-level alias would be a capture the build step rejects).
type Untyped = {
  kind(language: string, kind: string): number;
  parseFile(language: string, path: string): { write(content: string): void };
};

/** Rewrites `other.ts` through `jssgTransform` and reports what it returned. */
export const secondary = jssg({
  name: "secondary",
  language: "typescript",
  async transform(root, options) {
    const other = `${options.targetDir}/other.ts`;
    const changed = await jssgTransform(
      async (sub) => migrateText(sub.root().text()),
      other,
      "typescript",
    );
    const unchanged = await jssgTransform(
      async () => null,
      `${options.targetDir}/same.ts`,
      "typescript",
    );
    return {
      content: null,
      output: { file: posixPath(root.relativeFilename()), changed, unchanged },
    };
  },
});

/** Reports the options and root metadata a transform sees. */
export const options = jssg({
  name: "options",
  language: "typescript",
  transform(root, options) {
    return {
      content: null,
      output: {
        params: options.params,
        language: options.language,
        matches: options.matches === undefined ? "undefined" : "set",
        matrixValues: options.matrixValues === undefined ? "undefined" : "set",
        dryRun: options.dryRun,
        targetDirIsRoot: root.filename() === `${options.targetDir}/${root.relativeFilename()}`,
        relative: posixPath(root.relativeFilename()),
      },
    };
  },
});

/** `parse`, `kind`, `getMatch`, `replace`, and `commitEdits`. */
export const parseApi = jssg({
  name: "parse-api",
  language: "typescript",
  transform(root) {
    const program = root.root();
    const stringKind = (astGrep as unknown as Untyped).kind("typescript", "string");
    const strings = program.findAll(stringKind).map((node) => node.text());
    const parsed = parse("typescript", "let x = 1;").root().kind();
    const edits = program
      .findAll("oldApi($A)")
      .map((node) => node.replace(`newApi(${node.getMatch("A")!.text()}, ${strings.length})`));
    return { content: program.commitEdits(edits), output: { strings, parsed } };
  },
});

/** Renames `*.old.ts` beside itself and returns `null` (content kept). */
export const renameNull = jssg({
  name: "rename-null",
  language: "typescript",
  transform(root) {
    const name = root.filename().split(/[\\/]/u).pop()!;
    if (name.endsWith(".old.ts")) root.rename(name.replace(/\.old\.ts$/u, ".new.ts"));
    return null;
  },
});

/** Stages a `write()` to another file opened with `parseFile`. */
export const writeOther = jssg({
  name: "write-other",
  language: "typescript",
  transform(root, options) {
    (astGrep as unknown as Untyped)
      .parseFile("typescript", `${options.targetDir}/other.ts`)
      .write("export const written = 1;\n");
    return root.root().text();
  },
});

/** Returns something that is neither a string nor null. */
export const badResult = jssg({
  name: "bad-result",
  language: "typescript",
  transform(): string {
    return 42 as unknown as string;
  },
});

/** Reports a module-level counter: once per run in-process, once per file in the bridge. */
export const counter = jssg({
  name: "counter",
  language: "typescript",
  transform(root) {
    return {
      content: null,
      output: { file: posixPath(root.relativeFilename()), call: nextCall() },
    };
  },
});

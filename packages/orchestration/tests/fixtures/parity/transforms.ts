/**
 * Edge-case transforms for the cross-runtime JSSG parity test
 * (`tests/jssg-parity.e2e.test.ts`): every result rule, containment check,
 * and staging path a batch goes through. The workflow module imports this
 * one, so it must load in Node: `codemod:ast-grep` is imported inside the
 * transforms (the artifact keeps the dynamic import), never at the top.
 */
import { jssg } from "../../../src/index.ts";
import { migrateText, posixPath } from "../jssg/helpers.ts";
import encodingTransform from "./encoding.ts";

/** BOM, CRLF, and no trailing newline (`encoding.ts`, shared with the workflow engine). */
export const encoding = jssg({
  name: "encoding",
  language: "typescript",
  transform: encodingTransform,
});

type AstGrep = typeof import("codemod:ast-grep") & {
  parseFile(language: string, path: string): { write(content: string): void };
};

/**
 * Returns what `options.params.input` names: every shape of a transform
 * result (strings, nullish, wrong types, structured results, odd outputs).
 */
export const returns = jssg({
  name: "returns",
  language: "typescript",
  transform(root, options): string | null {
    const text = root.root().text();
    const results: Record<string, () => unknown> = {
      changed: () => migrateText(text),
      unchanged: () => text,
      null: () => null,
      undefined: () => undefined,
      int: () => 42,
      float: () => 1.5,
      bool: () => true,
      array: () => [text],
      "missing-output": () => ({ content: migrateText(text) }),
      "structured-null": () => ({ content: null, output: posixPath(root.relativeFilename()) }),
      "structured-bad-content": () => ({ content: 7, output: 1 }),
      "output-shapes": () => ({
        content: text,
        output: {
          file: posixPath(root.relativeFilename()),
          nested: { list: [1, "two", null, undefined, 2.5, -3, [true]] },
          missing: undefined,
          fn: () => 1,
          big: 2 ** 40,
          empty: {},
        },
      }),
    };
    const pick = results[String((options.params as { input?: unknown }).input)];
    if (pick === undefined) throw new Error(`unknown case ${String(options.params.input)}`);
    return pick() as string | null;
  },
});

/** Several edits in one file, a staged `write()`, and a staged `jssgTransform()`. */
export const multi = jssg({
  name: "multi",
  language: "typescript",
  async transform(root, options) {
    const program = root.root();
    const edits = program
      .findAll("oldApi($A)")
      .map((node) => node.replace(`newApi(${node.getMatch("A")!.text()})`));
    if (posixPath(root.relativeFilename()) === "main.ts") {
      // `parseFile` is exported by the runtime module but missing from the types.
      const astGrep = (await import("codemod:ast-grep")) as unknown as AstGrep;
      astGrep
        .parseFile("typescript", `${options.targetDir}/lib/plain.ts`)
        .write("export const written = true;\n");
      await astGrep.jssgTransform(
        async (sub) => migrateText(sub.root().text()),
        `${options.targetDir}/lib/other.ts`,
        "typescript",
      );
    }
    return { content: program.commitEdits(edits), output: edits.length };
  },
});

/** Renames each file into `options.params.input` (a directory, maybe outside the root). */
export const renameInto = jssg({
  name: "rename-into",
  language: "typescript",
  transform(root, options) {
    const name = root.filename().split(/[\\/]/u).pop()!;
    root.rename(`${options.targetDir}/${String(options.params.input)}/${name}`);
    return null;
  },
});

/** Writes through `jssgTransform()` to `options.params.input` (maybe outside the root). */
export const secondaryAt = jssg({
  name: "secondary-at",
  language: "typescript",
  async transform(_root, options) {
    const { jssgTransform } = await import("codemod:ast-grep");
    await jssgTransform(
      async () => "export const touched = 1;\n",
      `${options.targetDir}/${String(options.params.input)}`,
      "typescript",
    );
    return null;
  },
});

/**
 * Throws on `b.ts` (after `a.ts` succeeded). A plain `Error`: the bridge's
 * sandbox renders every thrown error as `Error: <message>`, dropping a
 * subclass name such as `TypeError` that the in-process runtimes keep.
 */
export const throwing = jssg({
  name: "throwing",
  language: "typescript",
  transform(root) {
    if (root.relativeFilename().endsWith("b.ts")) throw new Error("b.ts is not allowed");
    return migrateText(root.root().text());
  },
});

/** Throws for any file reaching it: the selector must skip every non-matching file. */
export const selected = jssg({
  name: "selected",
  language: "typescript",
  selector: { rule: { pattern: "oldApi($A)" } },
  transform(root) {
    const text = root.root().text();
    if (!text.includes("oldApi"))
      throw new Error(`selector let ${root.relativeFilename()} through`);
    return { content: migrateText(text), output: posixPath(root.relativeFilename()) };
  },
});

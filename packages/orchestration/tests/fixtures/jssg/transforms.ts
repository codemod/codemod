/**
 * Inline transforms for the bridge e2e tests. This module is only ever built
 * (`buildFile`), never imported: the tests take each definition's artifact by
 * name and bind it to a runnable of their own. It is typechecked with the
 * package, which is what proves the inline authoring types.
 */
import { jssg } from "../../../src/index.ts";
import { migrateText, posixPath } from "./helpers.ts";

/** Rewrites `oldApi` and reports the file; the structured result form. */
export const replace = jssg({
  name: "replace",
  language: "typescript",
  transform(root) {
    return {
      content: migrateText(root.root().text()),
      output: { file: posixPath(root.relativeFilename()) },
    };
  },
});

/**
 * Follows the `add()` call to its definition in another file through the
 * shared workspace index, edits that file with `write()`, and reports where
 * the definition lives. The selector matches only the calling file.
 */
export const semantic = jssg({
  name: "semantic",
  language: "typescript",
  semanticAnalysis: "workspace",
  selector: { rule: { pattern: "add($A, $B)" } },
  transform(root) {
    const file = posixPath(root.relativeFilename());
    const call = root
      .root()
      .findAll({ rule: { pattern: "add" } })
      .find((node) => node.parent()?.kind() === "call_expression");
    if (!call) return { content: null, output: { file, definition: null } };
    let definition = call.definition();
    for (
      let hop = 0;
      definition && definition.root.filename() === root.filename() && hop < 3;
      hop++
    ) {
      definition = definition.node.definition();
    }
    if (!definition) return { content: null, output: { file, definition: null } };
    definition.root.write(definition.root.root().text().replace("add", "sum"));
    return {
      content: null,
      output: { file, definition: posixPath(definition.root.relativeFilename()) },
    };
  },
});

/** Throws for any file the selector should have skipped; the legacy string result form. */
export const guarded = jssg({
  name: "guarded",
  language: "typescript",
  selector: { rule: { pattern: "oldApi($A)" } },
  transform: (root) => {
    const text = root.root().text();
    if (!text.includes("oldApi")) throw new Error("ran on a non-matching file");
    return migrateText(text);
  },
});

/** Edits every file but throws on `b.ts`, so a command over both must leave both untouched. */
export const failSecond = jssg({
  name: "fail-second",
  language: "typescript",
  async transform(root) {
    if (root.relativeFilename().endsWith("b.ts")) throw new Error("second file exploded");
    return migrateText(root.root().text());
  },
});

/** Every file renames itself to the same destination. */
export const conflict = jssg({
  name: "conflict",
  language: "typescript",
  transform(root) {
    root.rename("same.ts");
    return root.root().text();
  },
});

/** Renames `*.old.ts` to `*.new.ts` beside the file and rewrites its content. */
export const rename = jssg({
  name: "rename",
  language: "typescript",
  transform(root) {
    const name = root.filename().split(/[\\/]/u).pop()!;
    if (name.endsWith(".old.ts")) root.rename(name.replace(/\.old\.ts$/u, ".new.ts"));
    return migrateText(root.root().text());
  },
});

/** Tries to rename the file outside the target root. */
export const escape = jssg({
  name: "escape",
  language: "typescript",
  transform(root) {
    root.rename("../escaped.ts");
    return root.root().text();
  },
});

/** Never returns; used to prove that aborting a run kills the bridge process. */
export const hang = jssg({
  name: "hang",
  language: "typescript",
  transform(): string | null {
    for (;;) {
      // spin
    }
  },
});

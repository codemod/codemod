/**
 * Encoding parity (`tests/jssg-parity.e2e.test.ts`): what a transform sees
 * of a file with a byte order mark, CRLF line endings, or no trailing
 * newline, and what gets written back. Standalone (type-only imports, the
 * behaviour picked by the file name) so the workflow engine runs the same
 * file as a `js-ast-grep` step and the test compares it with the runtimes.
 *
 * - `*edit*`: `oldApi(...)` -> `newApi(...)` through `commitEdits`, with a
 *   trailing comment recording what the transform saw;
 * - `*same*`: returns `root().text()` unchanged;
 * - `*rename*`: `rename()`s the file to `*renamed*`, returns `null`;
 * - anything else: `null`.
 */
import type { SgRoot } from "codemod:ast-grep";
import type TS from "@codemod.com/jssg-types/langs/typescript";

export default function encoding(root: SgRoot<TS>): string | null {
  const program = root.root();
  const name = root.filename().split(/[\\/]/u).pop()!;
  const text = program.text();
  if (name.includes("same")) return text;
  if (name.includes("rename")) {
    root.rename(root.filename().replace(/rename([^/\\]*)$/u, "renamed$1"));
    return null;
  }
  if (!name.includes("edit")) return null;
  const calls = program.findAll({ rule: { pattern: "oldApi($$$A)" } });
  const edits = calls.flatMap((call) => {
    const callee = call.field("function");
    return callee === null ? [] : [callee.replace("newApi")];
  });
  const seen = {
    head: Array.from(text.slice(0, 2)).map((c) => c.charCodeAt(0)),
    length: text.length,
    crlf: text.includes("\r\n"),
    root: program.range(),
    first: calls[0]?.range() ?? null,
  };
  return `${program.commitEdits(edits)}/* seen ${JSON.stringify(seen)} */`;
}

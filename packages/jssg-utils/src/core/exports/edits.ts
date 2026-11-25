import type { SgNode } from "codemod/main";

/**
 * An edit that can leave the step it was planned in: plain JSON (a
 * `forEachFile` output, a step result), naming its file. Positions are the
 * byte offsets `node.range()` and `node.replace()` use, not string indexes.
 */
export type FileEdit = {
  /** The file, as `file.path` names it in a `forEachFile` callback (relative to the target). */
  path: string;
  /** Byte offsets in the file's UTF-8 text. */
  start: number;
  end: number;
  text: string;
};

/**
 * An edit replacing `node` with `text`, for applying in another step.
 * `path` is the node's file relative to the target (`root.relativeFilename()`),
 * falling back to its absolute filename.
 *
 * @example
 * ```ts
 * // Plan in one step, return the edits; apply them per file in the next.
 * const edits = calls.map((call) => planEdit(call, `await ${call.text()}`));
 * ```
 */
export function planEdit(node: SgNode, text: string): FileEdit {
  const root = node.getRoot();
  const range = node.range();
  return {
    path: root.relativeFilename() || root.filename(),
    start: range.start.index,
    end: range.end.index,
    text,
  };
}

/** An edit inserting `text` before (or after) `node`. */
export function planInsert(
  node: SgNode,
  text: string,
  where: "before" | "after" = "before",
): FileEdit {
  const edit = planEdit(node, text);
  return where === "before" ? { ...edit, end: edit.start } : { ...edit, start: edit.end };
}

/** The edits grouped by file, in file order of first appearance. */
export function groupByFile<E extends { path: string }>(edits: readonly E[]): Map<string, E[]> {
  const out = new Map<string, E[]>();
  for (const edit of edits) {
    const list = out.get(edit.path);
    if (list) list.push(edit);
    else out.set(edit.path, [edit]);
  }
  return out;
}

/** UTF-16 index of each UTF-8 byte offset in `text` (offsets inside a character round down). */
function byteToIndex(text: string): (byte: number) => number {
  const starts: number[] = [];
  const indexes: number[] = [];
  let byte = 0;
  for (let i = 0; i < text.length; ) {
    const code = text.codePointAt(i)!;
    starts.push(byte);
    indexes.push(i);
    byte += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    i += code > 0xffff ? 2 : 1;
  }
  starts.push(byte);
  indexes.push(text.length);
  return (offset) => {
    let lo = 0;
    let hi = starts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (starts[mid]! <= offset) lo = mid;
      else hi = mid - 1;
    }
    return indexes[lo]!;
  };
}

export type ApplyEditsOptions = {
  /**
   * When two edits overlap: `"outer"` keeps the one that starts first (and
   * is longer, at the same start) and drops the others; `"error"` throws.
   * @default "outer"
   */
  overlap?: "outer" | "error";
};

/**
 * Apply edits to a file's text: `file.text` in a `forEachFile` callback.
 * Edits for other files are ignored when `path` is given. Byte offsets are
 * converted to string indexes, so non-ASCII text before an edit does not
 * shift it. Insertions at one point keep their order.
 *
 * @example
 * ```ts
 * forEachFile({ include: files }, (file) => {
 *   const mine = plan.filter((edit) => edit.path === file.path);
 *   if (mine.length) file.write(applyEdits(file.text, mine));
 * });
 * ```
 */
export function applyEdits(
  text: string,
  edits: readonly FileEdit[],
  options: ApplyEditsOptions & { path?: string } = {},
): string {
  const mine =
    options.path === undefined ? [...edits] : edits.filter((e) => e.path === options.path);
  const ordered = mine
    .map((edit, order) => ({ edit, order }))
    // By start; at one start, insertions first (so "insert before x" and
    // "replace x" both apply), then the longer edit.
    .sort(
      (a, b) =>
        a.edit.start - b.edit.start ||
        Number(a.edit.start !== a.edit.end) - Number(b.edit.start !== b.edit.end) ||
        b.edit.end - a.edit.end ||
        a.order - b.order,
    );
  const kept: FileEdit[] = [];
  let reach = -1;
  for (const { edit } of ordered) {
    if (edit.start < reach) {
      if (options.overlap === "error") {
        throw new Error(
          `applyEdits: edits overlap at bytes ${edit.start}-${edit.end} in ${edit.path}`,
        );
      }
      continue;
    }
    kept.push(edit);
    reach = Math.max(reach, edit.end);
  }
  const index = byteToIndex(text);
  let out = "";
  let at = 0;
  for (const edit of kept) {
    const start = index(edit.start);
    out += text.slice(at, start) + edit.text;
    at = Math.max(at, index(edit.end));
  }
  return out + text.slice(at);
}

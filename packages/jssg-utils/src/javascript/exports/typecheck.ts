/**
 * A type-check gate for codemods that move or remove files: parse a
 * checker's output, keep the errors the codemod introduced, and map each one
 * to the file change that caused it, so the workflow can undo that change.
 *
 * The checker itself runs in a workflow `shell` step (the jssg sandbox does
 * not run processes): see "Type-check gate" in the JSSG data-flow docs.
 */

/** One diagnostic from `tsc` (or a checker with the same output format). */
export type TypeError = {
  /** The file, as the checker printed it (relative to where it ran). */
  file: string;
  line: number;
  column: number;
  /** `TS2307`. */
  code: string;
  message: string;
};

/** A file the codemod moved (`to` set) or removed (`to` null), relative to the checker's directory. */
export type FileChange = { from: string; to: string | null };

const PLAIN = /^(.+?)\((\d+),(\d+)\): error (TS\d+): (.*)$/;
const PRETTY = /^(.+?):(\d+):(\d+) - error (TS\d+): (.*)$/;

/**
 * The errors in a checker's output. Reads `tsc`'s plain format
 * (`src/a.ts(3,10): error TS2307: …`) and its `--pretty` one
 * (`src/a.ts:3:10 - error TS2307: …`); continuation lines are dropped.
 */
export function parseTypeErrors(output: string): TypeError[] {
  const errors: TypeError[] = [];
  for (const raw of output.split(/\r?\n/)) {
    // eslint-disable-next-line no-control-regex
    const line = raw.replace(/\u001b\[[0-9;]*m/g, "");
    const match = PLAIN.exec(line) ?? PRETTY.exec(line);
    if (!match) continue;
    errors.push({
      file: match[1]!.trim(),
      line: Number(match[2]),
      column: Number(match[3]),
      code: match[4]!,
      message: match[5]!.trim(),
    });
  }
  return errors;
}

/**
 * Errors in `after` that were not in `before` (same file, code and message;
 * line numbers are ignored, since the codemod moves lines).
 */
export function newTypeErrors(before: TypeError[], after: TypeError[]): TypeError[] {
  const key = (e: TypeError) => `${e.file}\u0000${e.code}\u0000${e.message}`;
  const seen = new Map<string, number>();
  for (const e of before) seen.set(key(e), (seen.get(key(e)) ?? 0) + 1);
  return after.filter((e) => {
    const left = seen.get(key(e)) ?? 0;
    if (left === 0) return true;
    seen.set(key(e), left - 1);
    return false;
  });
}

const EXTENSIONS = [".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"];

function normalize(path: string): string {
  const out: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  return (path.startsWith("/") ? "/" : "") + out.join("/");
}

function dirname(path: string): string {
  const at = path.lastIndexOf("/");
  return at < 0 ? "." : path.slice(0, at) || "/";
}

/** The module paths a relative specifier can mean: as written, with each extension, or a directory's index. */
function candidates(fromFile: string, specifier: string): string[] {
  const base = normalize(`${dirname(fromFile)}/${specifier}`);
  const stem = base.replace(/\.(m|c)?js$/, "");
  return [
    base,
    ...EXTENSIONS.map((ext) => stem + ext),
    ...EXTENSIONS.map((ext) => `${base}/index${ext}`),
  ];
}

/**
 * Which change caused each new error. An error is put on a change when:
 * - it says a module cannot be found (`TS2307`, `TS2305` "has no exported
 *   member", `TS2614`) and the module, resolved from the error's file, is a
 *   file that was removed or moved away; or
 * - it is in a file that was moved (its imports point at the old place).
 *
 * Errors with no cause found come back under `null`: the codemod's edits,
 * not a file change, introduced them (review those).
 *
 * Only relative specifiers are resolved; for an alias, pass `resolve` (for
 * example a map from the specifiers in the errors to files).
 *
 * @example
 * ```ts
 * const causes = typeErrorCauses(newTypeErrors(before, after), removedBarrels);
 * for (const [change, errors] of causes) if (change) restore(change);
 * ```
 */
export function typeErrorCauses(
  errors: TypeError[],
  changes: FileChange[],
  resolve?: (fromFile: string, specifier: string) => string | null,
): Map<FileChange | null, TypeError[]> {
  const byPath = new Map<string, FileChange>();
  for (const change of changes) byPath.set(normalize(change.from), change);
  const movedTo = new Map<string, FileChange>();
  for (const change of changes) if (change.to) movedTo.set(normalize(change.to), change);

  const out = new Map<FileChange | null, TypeError[]>();
  const add = (change: FileChange | null, error: TypeError) => {
    const list = out.get(change) ?? [];
    list.push(error);
    out.set(change, list);
  };
  for (const error of errors) {
    let cause: FileChange | null = null;
    const specifier = /['"]([^'"]+)['"]/.exec(error.message)?.[1];
    if (specifier && /^TS(2307|2305|2614|2792)$/.test(error.code)) {
      const resolved = resolve?.(error.file, specifier);
      const paths = resolved
        ? [normalize(resolved)]
        : specifier.startsWith(".")
          ? candidates(error.file, specifier)
          : [];
      cause = paths.map((p) => byPath.get(p)).find(Boolean) ?? null;
    }
    cause ??= movedTo.get(normalize(error.file)) ?? null;
    add(cause, error);
  }
  return out;
}

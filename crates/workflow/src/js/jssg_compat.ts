// `codemod-internal:jssg-compat`: a jssg transform module (the default
// export `transform(root, options)`, and optionally `getSelector`) run from a
// `forEachFile` callback, as workflow.yaml's `js-ast-grep` step ran it.
//
// The YAML translation uses it: workflow.yaml runs, and what
// `codemod workflow upgrade` writes for a transform a plain callback cannot
// call (one that uses `root.rename()` or `jssgTransform`); the `codemod`
// package ships its types (types/modules/jssg-compat.d.ts). Upgraded
// workflows call other transforms from plain callbacks.
// What it reproduces: the root the transform gets (with the step's semantic
// analysis), `options.params`, `options.matrixValues`, `options.language`,
// `options.targetDir` and `options.matches` (from `getSelector`, whose files
// without a match are skipped), a string, `null` or `{ edit, data }` result,
// `root.rename(path)`, and `jssgTransform` on other files.
//
// `jssgTransform` changes other files from a transform. The engine keeps
// those changes with the step's other unwritten changes (`__jssgCompatHost`,
// crates/workflow/src/tasks/for_each_file.rs): `begin` gives the file's text
// with what other files' transforms did to it so far, and holds the file
// until its transform is done; the changes the transform made to other files
// are kept only if it succeeds.

import { parse } from "codemod/ast-grep";

/** A jssg transform module, imported as a namespace. */
export interface JssgModule {
  default?: unknown;
  getSelector?: unknown;
}

/**
 * `__jssgCompatHost`, in `forEachFile` isolates that run one file at a time
 * per worker (crates/workflow/src/tasks/for_each_file.rs).
 */
interface CompatHost {
  /**
   * Start running a transform on the file at `path`: its text as the step
   * has it now. With `discard`, what the transform changes is dropped.
   */
  begin(path: string, discard: boolean): string | null;
}

/** The file a `forEachFile` callback gets. */
interface CallbackFile {
  readonly path: string;
  readonly text: string;
  /** The selector's language. */
  readonly language?: string;
  write(text: string): void;
  rename(path: string): void;
}

export interface JssgOptions {
  /** The language to parse with (default: the step selector's). */
  language?: string;
  params?: Record<string, unknown>;
  matrix?: Record<string, unknown> | null;
  /** Only files under this directory of the target. */
  basePath?: string;
  /** Run the transform, but leave the file as it was. */
  dryRun?: boolean;
}

interface Applied {
  /** No match for the transform's selector: the transform did not run. */
  skipped: boolean;
  /** The file's new text, when the transform changed it. */
  text?: string;
  /** Where the transform renamed the file to, relative to the target. */
  renamed?: string;
  data?: unknown;
}

// Typed loosely: the root is the same object on both hosts' `parse`.
// eslint-disable-next-line typescript-eslint/no-explicit-any -- an ast-grep root
type Root = any;

async function apply(
  module: JssgModule,
  file: CallbackFile,
  options: JssgOptions,
): Promise<Applied> {
  const transform = module.default;
  if (typeof transform !== "function") {
    throw new Error(`jssg codemod modules must export a function. ${typeof transform} found.`);
  }
  const language = options.language ?? file.language;
  const root: Root = language ? parse(file, { language }) : parse(file);
  const params = options.params ?? {};
  const transformOptions: Record<string, unknown> = {
    params,
    language: language ?? "",
    targetDir: targetDir(root, file),
  };
  if (options.matrix) transformOptions.matrixValues = options.matrix;
  if (options.dryRun) transformOptions.dryRun = true;
  if ("getSelector" in module) {
    if (typeof module.getSelector !== "function") {
      throw new Error(`jssg selector must be a function. ${typeof module.getSelector} found.`);
    }
    const selector = (module.getSelector as (options: unknown) => unknown)({ params });
    if (selector && typeof selector === "object") {
      const { rule, constraints, utils } = selector as Record<string, unknown>;
      const matches = root.root().findAll({ rule, constraints, utils });
      if (!matches.length) return { skipped: true };
      transformOptions.matches = matches;
    }
  }
  let renamed: string | undefined;
  // `root.rename(path)` renames the file, relative to its directory.
  const view = new Proxy(root, {
    get(target, key) {
      if (key === "rename") {
        return (to: string) => {
          if (renamed !== undefined) {
            throw new Error(
              "rename() has already been called for this file. It can only be called once.",
            );
          }
          renamed = renamedPath(file.path, to, transformOptions.targetDir as string);
        };
      }
      const value: unknown = Reflect.get(target, key);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
  const result: unknown = await (transform as (root: unknown, options: unknown) => unknown)(
    view,
    transformOptions,
  );
  let edit: unknown = result;
  let data: unknown;
  if (result !== null && typeof result === "object") {
    for (const key of Object.keys(result)) {
      if (key !== "edit" && key !== "data") {
        throw new Error(
          `A transform's result object takes only \`edit\` and \`data\`. Received \`${key}\``,
        );
      }
    }
    ({ edit, data } = result as { edit?: unknown; data?: unknown });
  }
  if (edit !== null && edit !== undefined && typeof edit !== "string") {
    throw new Error(
      `Codemod transform functions must return a string, null/undefined, or { edit, data }. Received ${typeof edit}`,
    );
  }
  const applied: Applied = { skipped: false, data };
  if (typeof edit === "string" && edit !== file.text) applied.text = edit;
  if (renamed !== undefined) applied.renamed = renamed;
  return applied;
}

/**
 * Run the transform on the callback's file and apply what it did; returns
 * its `data`, which the step collects into its outputs.
 */
export async function jssgFile(
  module: JssgModule,
  callbackFile: CallbackFile,
  options: JssgOptions = {},
): Promise<unknown> {
  if (!underBase(callbackFile.path, options.basePath)) return undefined;
  const file = begin(callbackFile, options.dryRun === true);
  const applied = await apply(module, file, options);
  if (options.dryRun) return applied.data;
  if (applied.text !== undefined) file.write(applied.text);
  if (applied.renamed !== undefined) file.rename(applied.renamed);
  return applied.data;
}

/**
 * For `shard`'s filter: `true` when the transform would change the file,
 * `"matched"` when its selector matched but it would not (such files are
 * kept only when no file would change), nothing otherwise. Changes nothing.
 */
export async function jssgWouldChange(
  module: JssgModule,
  file: CallbackFile,
  options: JssgOptions = {},
): Promise<true | "matched" | undefined> {
  if (!underBase(file.path, options.basePath)) return undefined;
  // As the YAML engine's shard prescan did: the transform sees a dry run.
  const applied = await apply(module, begin(file, true), { ...options, dryRun: true });
  if (applied.text !== undefined || applied.renamed !== undefined) return true;
  if (!applied.skipped && "getSelector" in module) return "matched";
  return undefined;
}

/**
 * The file with the text the step has for it now, which other files'
 * transforms may have changed with `jssgTransform`; `discard` drops what
 * this one changes. Without the engine's host (several files in flight per
 * worker, or the Node.js host), the callback's file as it is, and
 * `jssgTransform` on other files fails.
 */
function begin(file: CallbackFile, discard: boolean): CallbackFile {
  const host = (globalThis as { __jssgCompatHost?: CompatHost }).__jssgCompatHost;
  const text = host?.begin(file.path, discard) ?? null;
  if (text === null || text === file.text) return file;
  return {
    path: file.path,
    text,
    language: file.language,
    write: (next) => file.write(next),
    rename: (to) => file.rename(to),
  };
}

/** Whether `path` is under `base`, a directory of the target (all of it when not given). */
function underBase(path: string, base: string | undefined): boolean {
  const dir = (base ?? "")
    .replaceAll("\\", "/")
    .replace(/^\.\/?/, "")
    .replace(/\/+$/, "");
  return dir === "" || path.startsWith(`${dir}/`);
}

/** The target directory: the root's absolute filename without the file's path. */
function targetDir(root: Root, file: CallbackFile): string {
  const filename: unknown = typeof root.filename === "function" ? root.filename() : undefined;
  if (typeof filename !== "string") return ".";
  const normalized = filename.replaceAll("\\", "/");
  return normalized.endsWith(`/${file.path}`)
    ? filename.slice(0, filename.length - file.path.length - 1)
    : ".";
}

/** A path given to `root.rename()`, relative to the file's directory or absolute, as a path relative to the target. */
function renamedPath(path: string, to: string, target: string): string {
  const slashed = to.replaceAll("\\", "/");
  const base = target.replaceAll("\\", "/");
  if (slashed.startsWith("/") || /^[A-Za-z]:\//.test(slashed)) {
    return slashed.startsWith(`${base}/`) ? slashed.slice(base.length + 1) : slashed;
  }
  const parts = path.split("/").slice(0, -1);
  for (const part of slashed.split("/")) {
    if (part === "..") parts.pop();
    else if (part !== "." && part !== "") parts.push(part);
  }
  return parts.join("/");
}

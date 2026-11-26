// Fixture declarations for private helpers emitted by the 1.x upgrade translator.
// oxlint-disable triple-slash-reference
/// <reference path="workflow.d.ts" />

/**
 * Runs a jssg transform module (its default export `transform(root,
 * options)`, and optionally `getSelector`) from a `forEachFile` callback,
 * as workflow.yaml's `js-ast-grep` step ran it.
 *
 * `codemod workflow upgrade` uses it only for transforms a plain callback
 * cannot call: those that call `root.rename()` (in a callback, that is
 * `file.rename()`) or `jssgTransform`. Every other transform is called as
 * `transform(parse(file))` from the callback.
 */
declare module "codemod-internal:jssg-compat" {
  import type { JsonValue, WorkflowFile } from "codemod:workflow";

  /** A jssg transform module, imported as a namespace: `import * as transform from "./src/codemod.ts"`. */
  export interface JssgModule {
    readonly default?: unknown;
    readonly getSelector?: unknown;
  }

  export interface JssgOptions {
    /** The language to parse with (default: the step selector's). */
    language?: string;
    /** The transform's `options.params`. */
    params?: Readonly<Record<string, unknown>>;
    /** The transform's `options.matrixValues`. */
    matrix?: Readonly<Record<string, unknown>> | null;
    /** Only files under this directory of the target. */
    basePath?: string;
    /** Run the transform, but leave the file as it was. */
    dryRun?: boolean;
    /**
     * Codemod 1.x compatibility, in runs of a workflow.yaml: the workflow's
     * state, which the transform reads with `getState` from `codemod:workflow`.
     */
    v1State?: Readonly<Record<string, unknown>>;
  }

  /**
   * Run the transform on the callback's file and apply what it did (its new
   * text, `root.rename()`, and `jssgTransform` on other files). Resolves to
   * the transform's `data`, which the step lists in its `outputs`.
   *
   * @example
   * ```ts
   * import * as moveStyles from "./src/move-styles.ts";
   * import { jssgFile } from "codemod-internal:jssg-compat";
   *
   * await step(forEachFile({ include: "**\/*.less" }, (file) => jssgFile(moveStyles, file, { params })));
   * ```
   */
  export function jssgFile(
    module: JssgModule,
    file: WorkflowFile,
    options?: JssgOptions,
  ): Promise<JsonValue | undefined>;

  /**
   * For `shard`'s `filter`: whether the transform would change the file. It
   * changes nothing.
   */
  export function jssgWouldChange(
    module: JssgModule,
    file: WorkflowFile,
    options?: JssgOptions,
  ): Promise<boolean | undefined>;
}

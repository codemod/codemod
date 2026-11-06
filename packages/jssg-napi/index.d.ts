/** The changes one transform call staged through `write()` or `jssgTransform()` (opaque). */
export interface StagedChanges {
  readonly __staged: unique symbol;
}

/** Opaque native `SgRoot` (see `codemod:ast-grep` in `@codemod.com/jssg-types` for its API). */
export type NativeRoot = object;

/** One file's batch result (`FileOutcome` in `@codemod.com/orchestration`). */
export interface FileOutcome {
  path: string;
  edits: { path: string; content: string; renameTo?: string }[];
  output?: unknown;
}

/**
 * One planned JSSG batch: the bridge's batch rules (`crates/jssg-batch`).
 * The constructor throws the planning error.
 */
export interface JssgBatch {
  readonly module: string;
  readonly targetRoot: string;
  readonly params: Record<string, unknown>;
  /** Roots of files `start ..< start + contents.length`, parsed off-thread; `null` where the selector skips. */
  parse(start: number, contents: string[]): Promise<(NativeRoot | null)[]>;
  /** A fresh transform `options` object. */
  options(): {
    params: Record<string, unknown>;
    language: string;
    matches: undefined;
    matrixValues: undefined;
    dryRun: false;
    targetDir: string;
  };
  /**
   * File `index`'s `FileOutcome`, or `null` when it has no edits and no
   * output (`{ path, edits: [] }`); throws `JSSG failed for '<path>': ...`.
   */
  finish(
    index: number,
    root: NativeRoot,
    result: unknown,
    staged: StagedChanges | null,
  ): FileOutcome | null;
}

/** The addon's exports used by the engine. */
export interface Binding {
  SgRoot: abstract new () => NativeRoot;
  SgNode: abstract new () => object;
  /** `config`: JSON of `jssg_batch::BatchConfig`; `paths`: root-relative file paths. */
  JssgBatch: new (config: string, paths: string[]) => JssgBatch;
  parse(language: string, source: string): NativeRoot;
  parseAsync(language: string, source: string): NativeRoot;
  parseFile(language: string, path: string): NativeRoot;
  kind(language: string, kind: string): number;
  engineBegin(targetDir: string): void;
  /** `null` when the call staged nothing. */
  engineEnd(): StagedChanges | null;
  engineStageResult(path: string, root: NativeRoot, result: unknown): string | null;
  engineCheckWithinTarget(path: string, caller: string): void;
  engineTargetDir(): string | null;
  engineLanguageName(language: string): string;
  engineParseWithPath(
    language: string,
    source: string,
    filename: string,
    targetDir?: string | null,
  ): NativeRoot;
  /** Orchestration `Host.selectFiles` (`crates/orchestration-walker`); the request as JSON. */
  selectFiles(request: string): string[];
  /**
   * Orchestration `Host.fs.readSources`: verbatim (a leading BOM included);
   * `null` for a missing or non-UTF-8 file.
   */
  readSources(root: string, paths: string[]): (string | null)[];
}

export function bindingPath(env?: Record<string, string | undefined>): string;
export function isAvailable(env?: Record<string, string | undefined>): boolean;
export function loadBinding(): Binding;
export function currentTransform(): { targetDir: string; params: unknown } | undefined;
export function runTransform(
  targetDir: string,
  params: unknown,
  call: () => unknown,
): Promise<{ result: unknown; staged: StagedChanges | null }>;

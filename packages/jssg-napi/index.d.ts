/** A change a transform staged through `write()` or `jssgTransform()`. */
export interface StagedChange {
  path: string;
  content: string | null;
  renameTo: string | null;
}

/** Opaque native `SgRoot` (see `codemod:ast-grep` in `@codemod.com/jssg-types` for its API). */
export type NativeRoot = object;

export interface NativeSelector {
  test(root: NativeRoot): boolean;
}

/** The addon's exports used by the engine. */
export interface Binding {
  SgRoot: abstract new () => NativeRoot;
  SgNode: abstract new () => object;
  Selector: new (language: string, rule: unknown) => NativeSelector;
  parse(language: string, source: string): NativeRoot;
  parseAsync(language: string, source: string): NativeRoot;
  parseFile(language: string, path: string): NativeRoot;
  kind(language: string, kind: string): number;
  engineBegin(targetDir: string): void;
  engineEnd(): StagedChange[];
  engineStage(path: string, content: string | null, renameTo: string | null): void;
  engineCheckWithinTarget(path: string, caller: string): void;
  engineTargetDir(): string | null;
  engineRenameTo(root: NativeRoot): string | null;
  engineSource(root: NativeRoot): string;
  engineLanguageName(language: string): string;
  engineParseWithPath(
    language: string,
    source: string,
    filename: string,
    targetDir?: string | null,
  ): NativeRoot;
  engineParseBatch(
    language: string,
    targetDir: string,
    paths: string[],
    contents: string[],
    selector?: NativeSelector | null,
  ): Promise<(NativeRoot | null)[]>;
  /** Orchestration `Host.selectFiles` (`crates/orchestration-walker`); the request as JSON. */
  selectFiles(request: string): string[];
  /** Orchestration `Host.fs.readSources`; `null` for a missing or non-UTF-8 file. */
  readSources(root: string, paths: string[], stripBom: boolean): (string | null)[];
}

export function bindingPath(env?: Record<string, string | undefined>): string;
export function isAvailable(env?: Record<string, string | undefined>): boolean;
export function loadBinding(): Binding;
export function currentTransform(): { targetDir: string; params: unknown } | undefined;
export function runTransform(
  targetDir: string,
  params: unknown,
  call: () => unknown,
): Promise<{ result: unknown; staged: StagedChange[] }>;

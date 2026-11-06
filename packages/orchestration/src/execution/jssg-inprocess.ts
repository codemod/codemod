/**
 * In-process JSSG batch runner: a `JssgDispatch` for runtimes that execute
 * transforms in the same JS realm as the control plane (`node`, and any host
 * that provides a `JssgEngine`). It is the in-process twin of
 * `crates/execution-bridge/src/jssg.rs::transform_batch`, with the same
 * contract towards `executeJssgCommand`:
 *
 * - the artifact module is evaluated once per dispatcher (one dispatcher per
 *   run), cached by hash, and only when a file actually reaches a transform;
 * - every file is parsed from the content the host sent (snapshot semantics:
 *   no transform sees another's edits), and files the static selector does
 *   not match are skipped before any transform runs (`{ path, edits: [] }`);
 * - transforms run serially, one at a time per process (`engine.runTransform`
 *   serializes across concurrent commands), with the options the sandbox
 *   builds: `{ params, language, matches, matrixValues, dryRun, targetDir }`;
 * - results become `FileOutcome`s exactly as the bridge's `convert` does:
 *   the file's own edit when its content changed or it was renamed, staged
 *   `write()` / `jssgTransform()` edits, renames and secondary paths
 *   normalized to root-relative `/` paths after a containment check, and the
 *   structured `output`;
 * - it never writes the repository and never throws: failures are a `failed`
 *   completion (`cancelled` when the signal fired), and `executeJssgCommand`
 *   commits nothing.
 *
 * Portable: the engine (AST bindings and module loading) and the `Host`
 * (realpath/lstat for containment) are injected; nothing here imports Node.
 * A transform that never returns cannot be interrupted in-process; the
 * signal is checked between files.
 */
import { errorCode, getHost, type Host } from "../core/host.ts";
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  type BatchFile,
  type Edit,
  type FileOutcome,
  type JssgOperation,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";
import type { JssgDispatch } from "./jssg.ts";

/** An artifact as the request carries it plus its identity. */
export interface TransformArtifact {
  name: string;
  hash: string;
  source: string;
}

/** A change a transform staged through `write()` or `jssgTransform()`. */
export interface StagedChange {
  /** Absolute (or root-relative) path of the file the content is for. */
  path: string;
  /** `null`: `jssgTransform()` left the file unmodified (the path is still checked). */
  content: string | null;
  /** Absolute rename target recorded on the secondary root, if any. */
  renameTo: string | null;
}

/**
 * What a runtime provides to run transforms in-process. `Root` is the
 * engine's `SgRoot`; this runner only passes it to the transform and back.
 */
export interface JssgEngine<Root = unknown> {
  /** Evaluate an artifact module and return its default export. Called once per hash per dispatcher. */
  loadTransform(artifact: TransformArtifact, runId: string): Promise<unknown>;
  /** `options.language` (the language's display name); throws `invalid JSSG language ...`. */
  languageName(language: string): string;
  /** Compile `operation.selector`; throws `invalid JSSG selector: ...`. */
  compileSelector(language: string, selector: Json): unknown;
  /**
   * Parse target-root-relative files (filename = real path, `targetDir` set
   * for `relativeFilename()`), `null` where the compiled selector matches
   * nothing. May parse off the JS thread.
   */
  parseBatch(
    language: string,
    targetRoot: string,
    files: readonly BatchFile[],
    selector: unknown,
  ): Promise<(Root | null)[]>;
  /**
   * Run `call` as the only transform in flight in this realm, with
   * `write()`/`jssgTransform()` staged and contained to `targetRoot` and
   * `params` visible to `jssgTransform()`. Resolves with its awaited result
   * and the staged changes; rejects with what `call` threw.
   */
  runTransform(
    targetRoot: string,
    params: Record<string, Json>,
    call: () => unknown,
  ): Promise<{ result: unknown; staged: StagedChange[] }>;
  /** Absolute `rename()` target recorded on `root`. */
  renameTo(root: Root): string | null | undefined;
  /** The source `root` was parsed from. */
  source(root: Root): string;
}

export interface InProcessJssgOptions {
  engine: JssgEngine;
  /** Defaults to the installed host. */
  host?: Host;
  /** Files parsed ahead per batch while the previous batch transforms. Default 64. */
  chunkSize?: number;
  /**
   * Optional timers, phase name and milliseconds: `parse` (each chunk's
   * off-thread parse, overlapping transforms), `parse.wait` (time blocked on
   * it), `load`, `transform`, `convert`.
   */
  profile?: (phase: string, ms: number) => void;
}

let dispatchers = 0;

/** A `JssgDispatch` running transforms in this realm; create one per run. */
export function createInProcessJssgDispatch(options: InProcessJssgOptions): JssgDispatch {
  const { engine } = options;
  const chunkSize = Math.max(1, options.chunkSize ?? 64);
  const runId = `r${++dispatchers}`;
  const modules = new Map<string, Promise<unknown>>();
  const loadTransform = (artifact: TransformArtifact) => {
    let module = modules.get(artifact.hash);
    if (module === undefined) {
      module = engine.loadTransform(artifact, runId);
      modules.set(artifact.hash, module);
    }
    return module;
  };

  return async (request: OperationRequest, signal?: AbortSignal) => {
    const host = options.host ?? getHost();
    const now = () => host.now();
    const timed = <T>(phase: string, start: number, value: T): T => {
      options.profile?.(phase, now() - start);
      return value;
    };
    const failed = (message: string): OperationCompletion => ({
      protocolVersion: PROTOCOL_VERSION,
      commandId: request.commandId,
      status: signal?.aborted ? "cancelled" : "failed",
      error: { message },
    });
    const operation = request.operation as JssgOperation;
    const context = request.context ?? {};
    const files = context.files ?? [];
    try {
      if (operation.kind !== "jssg") throw new Error(`not a jssg operation: ${operation.kind}`);
      const targetRoot = canonicalDir(host, context.targetRoot);
      const source = context.artifact?.source;
      if (source === undefined) {
        throw new Error(
          `JSSG transform '${operation.transform.name}' has no artifact source in the request context`,
        );
      }
      const artifact = { ...operation.transform, source };
      const language = engine.languageName(operation.language);
      const selector =
        operation.selector === undefined
          ? undefined
          : engine.compileSelector(operation.language, operation.selector as unknown as Json);
      const params: Record<string, Json> =
        operation.input === undefined ? {} : { input: operation.input };

      const parse = (start: number) => {
        const chunk = files.slice(start, start + chunkSize);
        const begun = now();
        return engine
          .parseBatch(operation.language, targetRoot, chunk, selector)
          .then((roots) => timed("parse", begun, roots));
      };
      const outcomes: FileOutcome[] = [];
      let pending = files.length > 0 ? parse(0) : undefined;
      for (let start = 0; pending !== undefined; start += chunkSize) {
        const waited = now();
        const roots = await pending;
        timed("parse.wait", waited, undefined);
        // Parse the next chunk off-thread while this one transforms.
        pending = start + chunkSize < files.length ? parse(start + chunkSize) : undefined;
        for (const [offset, root] of roots.entries()) {
          if (signal?.aborted) {
            pending?.catch(() => {});
            return failed("cancelled");
          }
          const file = files[start + offset]!;
          if (root === null) {
            outcomes.push({ path: file.path, edits: [] });
            continue;
          }
          let begun = now();
          const transform = await loadTransform(artifact).then(
            (module) => timed("load", begun, module),
            (error: unknown) => {
              pending?.catch(() => {});
              throw new Error(`JSSG failed for '${file.path}': ${describeError(error)}`);
            },
          );
          begun = now();
          let ran: { result: unknown; staged: StagedChange[] };
          try {
            ran = await engine.runTransform(targetRoot, params, () => {
              if (typeof transform !== "function") {
                throw new Error(
                  `jssg codemod modules must export a function. ${typeof transform} found.`,
                );
              }
              return transform(root, {
                params,
                language,
                matches: undefined,
                matrixValues: undefined,
                dryRun: false,
                targetDir: targetRoot,
              });
            });
          } catch (error) {
            pending?.catch(() => {});
            throw new Error(`JSSG failed for '${file.path}': ${describeError(error)}`);
          }
          timed("transform", begun, undefined);
          begun = now();
          try {
            outcomes.push(convert(host, targetRoot, file, root, ran, engine));
          } catch (error) {
            pending?.catch(() => {});
            throw new Error(`JSSG failed for '${file.path}': ${(error as Error).message}`);
          }
          timed("convert", begun, undefined);
        }
      }
      return {
        protocolVersion: PROTOCOL_VERSION,
        commandId: request.commandId,
        status: "succeeded",
        output: { files: outcomes as unknown as Json },
      };
    } catch (error) {
      return failed((error as Error).message ?? String(error));
    }
  };
}

function describeError(error: unknown): string {
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  return String(error);
}

function canonicalDir(host: Host, value: string | undefined): string {
  if (value === undefined || !host.path.isAbsolute(value)) {
    throw new Error("targetRoot must be an absolute path");
  }
  let real: string;
  try {
    real = host.fs.realpath(value);
  } catch (error) {
    throw new Error(`failed to resolve targetRoot '${value}': ${(error as Error).message}`);
  }
  if (!host.fs.lstat(real).isDirectory) throw new Error(`targetRoot '${value}' is not a directory`);
  return real;
}

/** The sandbox's result rules (`extract_transform_output` + `process_transform_result`). */
function primaryResult(
  result: unknown,
  original: string,
  renameTo: string | undefined,
): { content: string | undefined; output: Json | undefined } {
  let content: unknown = result;
  let output: Json | undefined;
  if ((typeof result === "object" && result !== null) || typeof result === "function") {
    const object = result as { content?: unknown; output?: unknown };
    if (object.output === undefined) {
      throw new Error("Object codemod results must contain an 'output' field");
    }
    output = toJson(object.output);
    content = object.content;
  }
  if (typeof content === "string") {
    return {
      content: content !== original || renameTo !== undefined ? content : undefined,
      output,
    };
  }
  if (content === null || content === undefined) {
    return { content: renameTo !== undefined ? original : undefined, output };
  }
  throw new Error(
    `Codemod transform functions must return either a string or null/undefined. Received ${quickJsTypeName(content)}`,
  );
}

function toJson(value: unknown): Json {
  let text: string | undefined;
  try {
    text = JSON.stringify(value);
  } catch (error) {
    throw new Error(`Codemod output must be JSON: ${(error as Error).message}`);
  }
  if (text === undefined) throw new Error(`Codemod output must be JSON: got ${typeof value}`);
  return JSON.parse(text) as Json;
}

function quickJsTypeName(value: unknown): string {
  switch (typeof value) {
    case "number":
      return Number.isInteger(value) ? "int" : "float";
    case "boolean":
      return "bool";
    case "bigint":
      return "big_int";
    default:
      return typeof value;
  }
}

function convert(
  host: Host,
  root: string,
  file: BatchFile,
  sgRoot: unknown,
  ran: { result: unknown; staged: StagedChange[] },
  engine: JssgEngine,
): FileOutcome {
  const renameTo = engine.renameTo(sgRoot) ?? undefined;
  const primary = primaryResult(ran.result, file.content, renameTo);
  const edits: Edit[] = [];
  const push = (path: string, content: string | undefined, rename: string | undefined) => {
    if (content === undefined) return;
    const edit: Edit = { path, content };
    if (rename !== undefined) edit.renameTo = normalizeOutput(host, root, rename, "rename target");
    edits.push(edit);
  };
  push(file.path, primary.content, renameTo);
  for (const change of ran.staged) {
    const path = normalizeOutput(host, root, change.path, "secondary path");
    push(path, change.content ?? undefined, change.renameTo ?? undefined);
  }
  const outcome: FileOutcome = { path: file.path, edits };
  if (primary.output !== undefined) outcome.output = primary.output;
  return outcome;
}

/**
 * `normalize_output` from the bridge: the real path of `requested` (relative
 * to `root` unless absolute), once symlinks in its nearest existing ancestor
 * are resolved, must lie inside `root`; returns it root-relative with `/`.
 */
export function normalizeOutput(host: Host, root: string, requested: string, name: string): string {
  if (requested === "") throw new Error(`${name} must not be empty`);
  const { path } = host;
  const candidate = path.isAbsolute(requested) ? requested : `${root}${path.sep}${requested}`;
  const escape = () => new Error(`${name} '${candidate}' escapes the target root`);
  if (candidate.split(/[\\/]/u).includes("..")) throw escape();
  let existing = candidate;
  let real: string | undefined;
  for (;;) {
    try {
      real = host.fs.realpath(existing);
      break;
    } catch (error) {
      const code = errorCode(error);
      if (code !== "ENOENT" && code !== "ENOTDIR" && code !== "ELOOP") {
        throw new Error(`failed to resolve ${name} '${existing}': ${(error as Error).message}`);
      }
    }
    const parent = path.dirname(existing);
    if (parent === existing) throw escape();
    existing = parent;
  }
  const full = real + candidate.slice(existing.length);
  if (full !== root && !full.startsWith(root.endsWith(path.sep) ? root : root + path.sep)) {
    throw escape();
  }
  const parts = full
    .slice(root.length)
    .split(path.sep)
    .filter((part) => part !== "" && part !== ".");
  if (parts.some((part) => part.includes("\\"))) {
    throw new Error(`${name} '${candidate}' is not a plain path`);
  }
  if (parts.length === 0) throw new Error(`${name} resolves to the target root itself`);
  return parts.join("/");
}

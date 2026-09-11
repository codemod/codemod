/**
 * JSSG orchestration for one command, entirely in TypeScript:
 *
 * 1. resolve the target root beneath the working directory;
 * 2. open one persistent Rust worker with the script, language, semantic
 *    mode, and invocation input, and learn the language's extensions;
 * 3. enumerate and order the effective file set (definition applicability
 *    intersected with the invocation target, engine walker semantics);
 * 4. in workspace semantic mode, index that set before any transform;
 * 5. transform each file serially, stage its primary and secondary edits and
 *    renames, validate cross-file conflicts, and refresh the semantic index
 *    with the staged content;
 * 6. close the worker, then commit every staged edit, or report why not.
 *
 * Failure classification: anything before commit leaves the repository
 * unchanged and is `failed` (or `cancelled` when the signal fired); a commit
 * that stops part-way is `unknown` with the applied and remaining paths.
 */
import { readFileSync, realpathSync } from "node:fs";
import { resolve } from "node:path";
import { nullSink, type EventSink, type JssgPhase } from "./events.ts";
import type { Json } from "./json.ts";
import { PathEscapeError, resolveInsideRoot } from "./paths.ts";
import {
  PROTOCOL_VERSION,
  type CompletionStatus,
  type JssgOperation,
  type OperationCompletion,
} from "./protocol.ts";
import { CommitError, Staging, StagingConflictError } from "./staging.ts";
import { selectFiles } from "./walker.ts";
import { JssgWorker, WorkerExitError } from "./worker.ts";
import type { TransformResult } from "./worker-protocol.ts";

export interface JssgExecutionOptions {
  bin: string;
  /** Repository root: target roots resolve beneath it and definition globs are relative to it. */
  cwd: string;
  /** Directory the operation's relative `script` resolves against. */
  scriptRoot: string;
  commandId: string;
  operation: JssgOperation;
  signal?: AbortSignal;
  events?: EventSink;
  env?: Record<string, string>;
  /** Test hook: global git excludes file (`null` disables). */
  globalExcludes?: string | null;
}

/** A failure before commit, with the phase it happened in. */
class JssgFailure extends Error {
  constructor(
    readonly phase: JssgPhase,
    message: string,
    readonly detail: Record<string, Json> = {},
  ) {
    super(message);
    this.name = "JssgFailure";
  }
}

const decoder = new TextDecoder("utf-8", { fatal: true });

/**
 * Read a selected file. `undefined` means it vanished since enumeration or is
 * not valid UTF-8; both are skipped, as the workflow engine does.
 */
function readSource(absolute: string): string | undefined {
  let bytes: Buffer;
  try {
    bytes = readFileSync(absolute);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
  try {
    return decoder.decode(bytes);
  } catch {
    return undefined;
  }
}

export async function executeJssg(options: JssgExecutionOptions): Promise<OperationCompletion> {
  const { commandId, operation, signal } = options;
  const events = options.events ?? nullSink;
  const completion = (
    status: CompletionStatus,
    message: string,
    details: Json,
  ): OperationCompletion =>
    status === "succeeded"
      ? { protocolVersion: PROTOCOL_VERSION, commandId, status, output: details }
      : { protocolVersion: PROTOCOL_VERSION, commandId, status, error: { message, details } };

  let worker: JssgWorker | undefined;
  let staging: Staging | undefined;
  const outputs: Json[] = [];
  try {
    const cwd = realpathSync.native(resolve(options.cwd));
    const scriptRoot = resolve(options.scriptRoot);
    const targetRoot = resolveTargetRoot(cwd, operation.target?.root);
    staging = new Staging(targetRoot);

    worker = JssgWorker.spawn({ bin: options.bin, cwd, env: options.env, signal });
    events.emit({ type: "jssg.worker", commandId, pid: worker.pid });
    const opened = await request(worker, "open", {
      type: "open",
      protocolVersion: PROTOCOL_VERSION,
      script: operation.script,
      scriptRoot,
      language: operation.language,
      targetRoot,
      ...(operation.semanticAnalysis === undefined
        ? {}
        : { semanticAnalysis: operation.semanticAnalysis }),
      ...(operation.input === undefined ? {} : { input: operation.input }),
    });
    if (opened.type !== "opened")
      throw new JssgFailure("open", `unexpected worker reply ${opened.type}`);

    let files: string[];
    try {
      files = selectFiles({
        cwd,
        targetRoot,
        definition: { include: operation.include, exclude: operation.exclude },
        invocation: { include: operation.target?.include, exclude: operation.target?.exclude },
        extensions: opened.extensions,
        globalExcludes: options.globalExcludes,
      });
    } catch (error) {
      throw new JssgFailure("select", (error as Error).message);
    }
    events.emit({
      type: "jssg.progress",
      commandId,
      phase: "select",
      completed: files.length,
      total: files.length,
    });

    if (opened.semanticMode === "workspace") {
      let indexed = 0;
      for (const path of files) {
        const content = readSource(resolveInsideRoot(targetRoot, path, "selected file"));
        if (content === undefined) continue;
        await request(worker, "index", { type: "index", path, content }, path);
        events.emit({
          type: "jssg.progress",
          commandId,
          phase: "index",
          path,
          completed: ++indexed,
          total: files.length,
        });
      }
    }

    let completed = 0;
    for (const path of files) {
      completed += 1;
      if (staging.isRemoved(path)) {
        events.emit({
          type: "jssg.progress",
          commandId,
          phase: "transform",
          path,
          completed,
          total: files.length,
          skipped: "renamed away",
        });
        continue;
      }
      const content =
        staging.contentFor(path) ??
        readSource(resolveInsideRoot(targetRoot, path, "selected file"));
      if (content === undefined) continue;
      const reply = await request(worker, "transform", { type: "transform", path, content }, path);
      if (reply.type !== "transformed")
        throw new JssgFailure("transform", `unexpected worker reply ${reply.type}`, { path });
      const written = stage(staging, path, reply.result);
      if (reply.result.output !== undefined) outputs.push(reply.result.output);
      if (opened.semanticMode !== null) {
        for (const edit of written) {
          await request(
            worker,
            "index",
            { type: "index", path: edit.path, content: edit.content },
            edit.path,
          );
        }
      }
      events.emit({
        type: "jssg.progress",
        commandId,
        phase: "transform",
        path,
        completed,
        total: files.length,
      });
    }
    await worker.close();
  } catch (error) {
    worker?.kill();
    if (signal?.aborted) {
      return completion("cancelled", `cancelled before commit: ${(error as Error).message}`, {
        phase: error instanceof JssgFailure ? error.phase : "transform",
        committed: false,
      });
    }
    if (error instanceof JssgFailure) {
      return completion("failed", error.message, { phase: error.phase, ...error.detail });
    }
    return completion("failed", (error as Error).message, { phase: "transform" });
  }

  if (signal?.aborted) {
    return completion("cancelled", "cancelled before commit", {
      phase: "commit",
      committed: false,
    });
  }
  const plan = staging.plan();
  events.emit({
    type: "jssg.progress",
    commandId,
    phase: "commit",
    completed: 0,
    total: plan.writes.length + plan.deletes.length,
  });
  try {
    const report = staging.commit(signal);
    events.emit({
      type: "jssg.progress",
      commandId,
      phase: "commit",
      completed: report.written.length + report.deleted.length,
      total: report.written.length + report.deleted.length,
    });
  } catch (error) {
    if (error instanceof CommitError) {
      return completion("unknown", error.message, { ...error.detail });
    }
    return completion("unknown", `commit failed: ${(error as Error).message}`, { phase: "commit" });
  }
  return completion("succeeded", "", outputs);
}

function resolveTargetRoot(cwd: string, root: string | undefined): string {
  let resolved: string;
  try {
    resolved = root === undefined ? cwd : resolveInsideRoot(cwd, root, "target root");
  } catch (error) {
    throw new JssgFailure("select", (error as Error).message);
  }
  let real: string;
  try {
    real = realpathSync.native(resolved);
  } catch (error) {
    throw new JssgFailure(
      "select",
      `failed to resolve JSSG target root '${root ?? "."}': ${(error as Error).message}`,
    );
  }
  return real;
}

async function request(
  worker: JssgWorker,
  phase: JssgPhase,
  message: Parameters<JssgWorker["request"]>[0],
  path?: string,
) {
  let reply: Awaited<ReturnType<JssgWorker["request"]>>;
  try {
    reply = await worker.request(message);
  } catch (error) {
    if (error instanceof WorkerExitError) {
      throw new JssgFailure(phase, error.message, path === undefined ? {} : { path });
    }
    throw new JssgFailure(phase, (error as Error).message, path === undefined ? {} : { path });
  }
  if (reply.type === "error") {
    throw new JssgFailure(phase, reply.message, {
      ...(path === undefined ? {} : { path }),
      fatal: reply.fatal,
    });
  }
  return reply;
}

function stage(staging: Staging, path: string, result: TransformResult) {
  try {
    return staging.apply(path, result);
  } catch (error) {
    if (error instanceof StagingConflictError) {
      throw new JssgFailure("stage", error.message, { ...error.detail, origin: path });
    }
    if (error instanceof PathEscapeError) {
      throw new JssgFailure("stage", error.message, { path, escaped: error.path });
    }
    throw error;
  }
}

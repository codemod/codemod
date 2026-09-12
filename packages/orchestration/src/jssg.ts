/**
 * One JSSG command, in TypeScript around one bridge process:
 *
 * 1. look up the built transform artifact the operation names by hash;
 * 2. resolve the target root beneath the working directory and select the
 *    effective file set (definition applicability intersected with the
 *    invocation target, engine walker semantics), reading every file;
 * 3. send the artifact source and the whole batch to one
 *    `butterflow-execution-bridge` process, which verifies the hash, indexes
 *    the batch for workspace semantics, skips files the static selector does
 *    not match, and transforms the rest from the content it was given
 *    (snapshot semantics: no transform sees another's edits);
 * 4. validate the returned edits, check cross-file conflicts, then commit.
 *
 * Nothing touches the repository before step 4's commit. Failures before it
 * are `failed` (or `cancelled` when the signal fired) with the repository
 * unchanged; a commit that stops part-way is `unknown` with the applied and
 * remaining paths.
 */
import { mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnBridge } from "./bridge.ts";
import type { ArtifactStore } from "./build.ts";
import type { EventSink } from "./events.ts";
import { comparePaths, selectFiles } from "./files.ts";
import type { Json } from "./json.ts";
import { lexists, resolveInsideRoot } from "./paths.ts";
import {
  PROTOCOL_VERSION,
  isFileOutcomes,
  isRecord,
  type BatchFile,
  type CompletionStatus,
  type FileOutcome,
  type JssgOperation,
  type OperationCompletion,
} from "./protocol.ts";

export interface JssgExecutionOptions {
  bin: string;
  /** Repository root: target roots resolve beneath it and definition globs are relative to it. */
  cwd: string;
  /** Built artifacts by hash; the operation's `transform.hash` must be present. */
  artifacts?: ArtifactStore;
  commandId: string;
  operation: JssgOperation;
  signal?: AbortSignal;
  events?: EventSink;
  env?: Record<string, string>;
  /** Test hook: global git excludes file (`null` disables). */
  globalExcludes?: string | null;
}

const decoder = new TextDecoder("utf-8", { fatal: true });

/**
 * Read a selected file. `undefined` means it vanished since selection or is
 * not valid UTF-8; both are skipped, as the workflow engine does.
 */
function readSource(absolute: string): string | undefined {
  try {
    return decoder.decode(readFileSync(absolute));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT" || error instanceof TypeError) {
      return undefined;
    }
    throw error;
  }
}

export async function executeJssg(options: JssgExecutionOptions): Promise<OperationCompletion> {
  const { commandId, operation, signal } = options;
  const fail = (
    status: Exclude<CompletionStatus, "succeeded">,
    message: string,
    details: Json,
  ): OperationCompletion => ({
    protocolVersion: PROTOCOL_VERSION,
    commandId,
    status,
    error: { message, details },
  });

  const artifact = options.artifacts?.get(operation.transform.hash);
  if (artifact === undefined) {
    return fail(
      "failed",
      `no built artifact for jssg '${operation.transform.name}' (hash ${operation.transform.hash.slice(0, 12)}); load the workflow with loadWorkflow() or codemod-workflow and pass its artifacts to BridgeExecutor`,
      { phase: "artifact" },
    );
  }

  let targetRoot: string;
  let files: BatchFile[];
  try {
    const cwd = realpathSync.native(resolve(options.cwd));
    targetRoot = realpathSync.native(
      operation.target?.root === undefined
        ? cwd
        : resolveInsideRoot(cwd, operation.target.root, "target root"),
    );
    files = selectFiles({
      cwd,
      targetRoot,
      language: operation.language,
      definition: { include: operation.include, exclude: operation.exclude },
      invocation: { include: operation.target?.include, exclude: operation.target?.exclude },
      globalExcludes: options.globalExcludes,
    }).flatMap((path) => {
      const content = readSource(join(targetRoot, path));
      return content === undefined ? [] : [{ path, content }];
    });
  } catch (error) {
    return fail("failed", (error as Error).message, { phase: "select" });
  }

  const completion = await spawnBridge(
    { bin: options.bin, cwd: options.cwd, env: options.env, events: options.events },
    {
      protocolVersion: PROTOCOL_VERSION,
      commandId,
      operation,
      context: { targetRoot, files, artifact: { source: artifact.source } },
    },
    signal,
  );
  // The bridge never writes, so anything short of success leaves the
  // repository unchanged.
  if (completion.status !== "succeeded") {
    return fail(signal?.aborted ? "cancelled" : "failed", completion.error.message, {
      phase: "transform",
    });
  }
  const outcomes = isRecord(completion.output) ? completion.output.files : undefined;
  if (
    !isFileOutcomes(outcomes) ||
    outcomes.length !== files.length ||
    outcomes.some((outcome, index) => outcome.path !== files[index]!.path)
  ) {
    return fail("failed", "bridge returned an invalid batch result", { phase: "transform" });
  }

  let staged: Staged;
  try {
    staged = stage(targetRoot, outcomes);
  } catch (error) {
    return fail("failed", (error as Error).message, { phase: "stage" });
  }
  if (signal?.aborted) return fail("cancelled", "cancelled before commit", { phase: "commit" });
  const applied: string[] = [];
  const steps = [
    ...staged.writes.map(([path, content]) => ({
      path,
      run: () => write(targetRoot, path, content),
    })),
    ...staged.deletes.map((path) => ({
      path,
      run: () => rmSync(join(targetRoot, path), { force: true }),
    })),
  ];
  for (const [index, step] of steps.entries()) {
    try {
      step.run();
    } catch (error) {
      return fail(
        "unknown",
        `commit failed at '${step.path}' after ${index} of ${steps.length} files: ${(error as Error).message}`,
        {
          phase: "commit",
          applied,
          failed: step.path,
          remaining: steps.slice(index).map((s) => s.path),
        },
      );
    }
    applied.push(step.path);
  }
  return {
    protocolVersion: PROTOCOL_VERSION,
    commandId,
    status: "succeeded",
    output: outcomes.flatMap((outcome) => (outcome.output === undefined ? [] : [outcome.output])),
  };
}

function write(root: string, path: string, content: string): void {
  const absolute = join(root, path);
  mkdirSync(dirname(absolute), { recursive: true });
  writeFileSync(absolute, content);
}

interface Staged {
  /** Destination path and content, in commit order. */
  writes: [string, string][];
  /** Rename sources to remove after every write, in commit order. */
  deletes: string[];
}

/**
 * Merge every outcome's edits into one write set, proving each path lies
 * beneath the root, and reject what snapshot semantics cannot reconcile:
 * two edits to one destination, a source renamed twice, a write to a path
 * another edit renames away, and a rename onto a file that exists unless that
 * file is itself renamed away. Throws before anything is written.
 */
function stage(root: string, outcomes: FileOutcome[]): Staged {
  const writes = new Map<string, { content: string; origin: string; renamed: boolean }>();
  const sources = new Map<string, string>();
  for (const { path: origin, edits } of outcomes) {
    for (const edit of edits) {
      resolveInsideRoot(root, edit.path, "edited file");
      const destination = edit.renameTo ?? edit.path;
      if (edit.renameTo !== undefined) {
        resolveInsideRoot(root, edit.renameTo, "rename target");
        const earlier = sources.get(edit.path);
        if (earlier !== undefined) {
          throw new Error(`'${edit.path}' is renamed by both '${earlier}' and '${origin}'`);
        }
        sources.set(edit.path, origin);
      }
      const existing = writes.get(destination);
      if (existing) {
        throw new Error(`'${destination}' is written by both '${existing.origin}' and '${origin}'`);
      }
      writes.set(destination, {
        content: edit.content,
        origin,
        renamed: edit.renameTo !== undefined,
      });
    }
  }
  for (const [destination, { origin, renamed }] of writes) {
    const renamer = sources.get(destination);
    if (!renamed && renamer !== undefined) {
      throw new Error(
        `'${destination}' is renamed away by '${renamer}' and written by '${origin}'`,
      );
    }
    if (renamed && renamer === undefined && lexists(join(root, destination))) {
      throw new Error(`'${origin}' renames onto '${destination}', which already exists`);
    }
  }
  return {
    writes: [...writes]
      .map(([path, { content }]): [string, string] => [path, content])
      .sort(([a], [b]) => comparePaths(a, b)),
    deletes: [...sources.keys()].filter((path) => !writes.has(path)).sort(comparePaths),
  };
}

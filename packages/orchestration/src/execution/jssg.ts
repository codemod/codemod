/**
 * One JSSG command, in TypeScript around one transform batch:
 *
 * 1. look up the built transform artifact the operation names by hash;
 * 2. resolve the target root beneath the working directory and select the
 *    effective file set (definition applicability intersected with the
 *    invocation target, engine walker semantics), reading every file;
 * 3. hand the artifact source and the whole batch to `dispatch`, which
 *    returns one `FileOutcome` per file (snapshot semantics: no transform
 *    sees another's edits). The bridge runtime dispatches to one
 *    `butterflow-execution-bridge` process (`bridge.ts::executeJssg`), which
 *    verifies the hash, indexes the batch for workspace semantics, skips
 *    files the static selector does not match, and transforms the rest;
 *    in-process runtimes dispatch to their own batch runner with the same
 *    contract;
 * 4. validate the returned edits, check cross-file conflicts, then commit.
 *
 * Nothing touches the repository before step 4's commit. Failures before it
 * are `failed` (or `cancelled` when the signal fired) with the repository
 * unchanged; a commit that stops part-way is `unknown` with the applied and
 * remaining paths. Every filesystem and path operation goes through the
 * `Host` (`core/host.ts`), so this module runs unchanged outside Node.
 */
import type { ArtifactStore } from "../bundle/build.ts";
import { comparePaths, readSelectedSources, selectFiles } from "./files.ts";
import { getHost, type Host } from "../core/host.ts";
import type { Json } from "../core/json.ts";
import { lexists, resolveInsideRoot } from "../core/paths.ts";
import {
  PROTOCOL_VERSION,
  isFileOutcomes,
  isRecord,
  type BatchFile,
  type CompletionStatus,
  type FileOutcome,
  type JssgOperation,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";

/**
 * Runs one transform batch: `request.context` carries `targetRoot`, the
 * selected `files` with their content, and the artifact `source`. It must
 * never write to the repository and never throw; a successful completion's
 * `output` is `{ files: FileOutcome[] }` in request order. `signal` aborts.
 */
export type JssgDispatch = (
  request: OperationRequest,
  signal?: AbortSignal,
) => Promise<OperationCompletion>;

export interface JssgCommandOptions {
  /** Repository root: target roots resolve beneath it and definition globs are relative to it. */
  cwd: string;
  /** Built artifacts by hash; the operation's `transform.hash` must be present. */
  artifacts?: ArtifactStore;
  commandId: string;
  operation: JssgOperation;
  signal?: AbortSignal;
  /** Executes the transform batch (step 3). */
  dispatch: JssgDispatch;
  /** Filesystem and paths. Defaults to the installed host. */
  host?: Host;
  /** Test hook: global git excludes file (`null` disables). */
  globalExcludes?: string | null;
  /** Optional timers in milliseconds: `select` (walk + reads), `stage`, `commit`. */
  profile?: (phase: string, ms: number) => void;
}

export async function executeJssgCommand(
  options: JssgCommandOptions,
): Promise<OperationCompletion> {
  const { commandId, operation, signal } = options;
  const host = options.host ?? getHost();
  const { join } = host.path;
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

  const { profile } = options;
  let mark = profile === undefined ? 0 : host.now();
  const lap = (phase: string) => {
    if (profile === undefined) return;
    const now = host.now();
    profile(phase, now - mark);
    mark = now;
  };

  let targetRoot: string;
  let files: BatchFile[];
  try {
    const cwd = host.fs.realpath(host.path.resolve(options.cwd));
    targetRoot = host.fs.realpath(
      operation.target?.root === undefined
        ? cwd
        : resolveInsideRoot(cwd, operation.target.root, "target root", host),
    );
    const paths = selectFiles({
      cwd,
      targetRoot,
      language: operation.language,
      definition: { include: operation.include, exclude: operation.exclude },
      invocation: { include: operation.target?.include, exclude: operation.target?.exclude },
      globalExcludes: options.globalExcludes,
      host,
    });
    files = readSelectedSources(host, targetRoot, paths);
  } catch (error) {
    return fail("failed", (error as Error).message, { phase: "select" });
  }
  lap("select");

  let completion: OperationCompletion;
  try {
    completion = await options.dispatch(
      {
        protocolVersion: PROTOCOL_VERSION,
        commandId,
        operation,
        context: { targetRoot, files, artifact: { source: artifact.source } },
      },
      signal,
    );
  } catch (error) {
    // A dispatcher must not throw; treat it as a transform failure (nothing was written).
    completion = {
      protocolVersion: PROTOCOL_VERSION,
      commandId,
      status: "failed",
      error: { message: (error as Error).message ?? String(error) },
    };
  }
  // The batch runner never writes, so anything short of success leaves the
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

  mark = profile === undefined ? 0 : host.now();
  let staged: Staged;
  try {
    staged = stage(host, targetRoot, outcomes);
  } catch (error) {
    return fail("failed", (error as Error).message, { phase: "stage" });
  }
  lap("stage");
  if (signal?.aborted) return fail("cancelled", "cancelled before commit", { phase: "commit" });
  const applied: string[] = [];
  const steps = [
    ...staged.writes.map(([path, content]) => ({
      path,
      run: () => write(host, targetRoot, path, content),
    })),
    ...staged.deletes.map((path) => ({
      path,
      run: () => host.fs.rm(join(targetRoot, path), { force: true }),
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
  lap("commit");
  return {
    protocolVersion: PROTOCOL_VERSION,
    commandId,
    status: "succeeded",
    output: outcomes.flatMap((outcome) => (outcome.output === undefined ? [] : [outcome.output])),
  };
}

function write(host: Host, root: string, path: string, content: string): void {
  const absolute = host.path.join(root, path);
  host.fs.mkdir(host.path.dirname(absolute), { recursive: true });
  host.fs.writeText(absolute, content);
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
function stage(host: Host, root: string, outcomes: FileOutcome[]): Staged {
  const writes = new Map<string, { content: string; origin: string; renamed: boolean }>();
  const sources = new Map<string, string>();
  for (const { path: origin, edits } of outcomes) {
    for (const edit of edits) {
      resolveInsideRoot(root, edit.path, "edited file", host);
      const destination = edit.renameTo ?? edit.path;
      if (edit.renameTo !== undefined) {
        resolveInsideRoot(root, edit.renameTo, "rename target", host);
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
    if (renamed && renamer === undefined && lexists(host.path.join(root, destination), host.fs)) {
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

/**
 * Staged edits for one JSSG command. Every transform result is applied to an
 * in-memory staged state first; nothing reaches disk until `commit()`, which
 * runs only after every selected file has been transformed and validated.
 *
 * Staged state:
 * - `writes`: destination path -> content and the file whose result produced it
 * - `deletes`: original paths of renamed files, removed after all writes
 *
 * Rules, applied in file order (primary result first, then secondary results
 * in the order the sandbox produced them):
 * - A later file's transform reads the staged content of its own path, so an
 *   earlier secondary edit to it is chained into its input and its primary
 *   result supersedes that staged write.
 * - Any other second write to the same destination is a conflict: two
 *   secondary edits of one file, or two renames onto one path.
 * - A rename source may be renamed only once, and a rename destination may
 *   not be an existing file unless that file was itself renamed away.
 * - A file that was renamed away earlier is skipped when its turn comes.
 *
 * Commit writes each destination through a sibling temp file plus an atomic
 * rename, then removes rename sources. Each file is atomic; the set is not: a
 * failure mid-commit is reported as `unknown` with the applied and remaining
 * paths. There is no cross-file transaction on ordinary filesystems.
 */
import { chmodSync, mkdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { randomBytes } from "node:crypto";
import type { Json } from "./json.ts";
import { lexists, resolveInsideRoot } from "./paths.ts";
import { comparePaths } from "./walker.ts";
import type { FileResult, TransformResult } from "./worker-protocol.ts";

export interface StagedWrite {
  content: string;
  /** The selected file whose transform produced this write. */
  origin: string;
}

export class StagingConflictError extends Error {
  constructor(
    message: string,
    readonly detail: { path: string; origin: string; conflictingOrigin?: string },
  ) {
    super(message);
    this.name = "StagingConflictError";
  }
}

export interface CommitDetail {
  phase: "commit";
  applied: string[];
  failed: string;
  remaining: string[];
  aborted: boolean;
}

export class CommitError extends Error {
  constructor(
    message: string,
    readonly detail: CommitDetail,
  ) {
    super(message);
    this.name = "CommitError";
  }
}

export interface CommitReport {
  written: string[];
  deleted: string[];
}

export class Staging {
  readonly writes = new Map<string, StagedWrite>();
  readonly deletes = new Set<string>();

  /** `root` is the real absolute target root. */
  constructor(readonly root: string) {}

  /** Staged content for `path`, or `undefined` when disk is current. */
  contentFor(path: string): string | undefined {
    return this.writes.get(path)?.content;
  }

  /** True when an earlier result renamed `path` away. */
  isRemoved(path: string): boolean {
    return this.deletes.has(path) && !this.writes.has(path);
  }

  /**
   * Stage every edit in `result`, which the transform of `origin` produced.
   * Returns the destinations written, for the host to re-index. Throws
   * `StagingConflictError` or `PathEscapeError`; on a throw nothing from this
   * result has been staged.
   */
  apply(origin: string, result: TransformResult): { path: string; content: string }[] {
    const entries: { path: string; result: FileResult }[] = [
      { path: origin, result: result.primary },
      ...result.secondary,
    ];
    const plan: { source: string; destination: string; content: string }[] = [];
    for (const entry of entries) {
      if (entry.result.kind !== "modified") continue;
      resolveInsideRoot(this.root, entry.path, "edited file");
      const destination = entry.result.renameTo ?? entry.path;
      if (entry.result.renameTo !== undefined) {
        resolveInsideRoot(this.root, entry.result.renameTo, "rename target");
      }
      plan.push({ source: entry.path, destination, content: entry.result.content });
    }
    // Validate against a snapshot so a conflict leaves the staging untouched.
    const writes = new Map(this.writes);
    const deletes = new Set(this.deletes);
    for (const step of plan) {
      if (step.source === step.destination) {
        this.stageWrite(writes, deletes, origin, step.destination, step.content);
      } else {
        this.stageRename(writes, deletes, origin, step.source, step.destination, step.content);
      }
    }
    this.writes.clear();
    for (const [path, write] of writes) this.writes.set(path, write);
    this.deletes.clear();
    for (const path of deletes) this.deletes.add(path);
    return plan.map((step) => ({ path: step.destination, content: step.content }));
  }

  private stageWrite(
    writes: Map<string, StagedWrite>,
    deletes: Set<string>,
    origin: string,
    path: string,
    content: string,
  ): void {
    if (deletes.has(path) && !writes.has(path)) {
      throw new StagingConflictError(
        `'${path}' was renamed away by an earlier result and is written again by '${origin}'`,
        { path, origin },
      );
    }
    const existing = writes.get(path);
    if (existing && !(path === origin && existing.origin !== origin)) {
      throw new StagingConflictError(
        `'${path}' is written by both '${existing.origin}' and '${origin}'`,
        { path, origin, conflictingOrigin: existing.origin },
      );
    }
    writes.set(path, { content, origin });
  }

  private stageRename(
    writes: Map<string, StagedWrite>,
    deletes: Set<string>,
    origin: string,
    source: string,
    destination: string,
    content: string,
  ): void {
    if (deletes.has(source) && !writes.has(source)) {
      throw new StagingConflictError(
        `'${source}' was already renamed away before '${origin}' renamed it to '${destination}'`,
        { path: source, origin },
      );
    }
    const existing = writes.get(destination);
    if (existing) {
      throw new StagingConflictError(
        `'${origin}' renames '${source}' to '${destination}', which '${existing.origin}' already writes`,
        { path: destination, origin, conflictingOrigin: existing.origin },
      );
    }
    if (
      !deletes.has(destination) &&
      lexists(resolveInsideRoot(this.root, destination, "rename target"))
    ) {
      throw new StagingConflictError(
        `'${origin}' renames '${source}' to '${destination}', which already exists`,
        { path: destination, origin },
      );
    }
    writes.delete(source);
    deletes.delete(destination);
    writes.set(destination, { content, origin });
    if (lexists(resolveInsideRoot(this.root, source, "renamed file"))) deletes.add(source);
  }

  /** Paths this commit will touch, in commit order. */
  plan(): { writes: string[]; deletes: string[] } {
    return {
      writes: [...this.writes.keys()].sort(comparePaths),
      deletes: [...this.deletes].filter((path) => !this.writes.has(path)).sort(comparePaths),
    };
  }

  /**
   * Apply every staged write, then every deletion. Each path is re-validated
   * against the root immediately before it is touched. Checks `signal`
   * between files and stops with `CommitError` (aborted) if it fired.
   */
  commit(signal?: AbortSignal): CommitReport {
    const { writes, deletes } = this.plan();
    const applied: string[] = [];
    const remainingAfter = (index: number, phase: "write" | "delete") =>
      phase === "write" ? [...writes.slice(index + 1), ...deletes] : deletes.slice(index + 1);
    const fail = (path: string, remaining: string[], cause: string, aborted: boolean) =>
      new CommitError(
        aborted
          ? `commit aborted after ${applied.length} of ${writes.length + deletes.length} files`
          : `commit failed at '${path}' after ${applied.length} of ${writes.length + deletes.length} files: ${cause}`,
        { phase: "commit", applied: [...applied], failed: path, remaining, aborted },
      );
    writes.forEach((path, index) => {
      if (signal?.aborted)
        throw fail(path, [path, ...remainingAfter(index, "write")], "aborted", true);
      const absolute = resolveInsideRoot(this.root, path, "staged file");
      const temp = `${absolute}.${randomBytes(6).toString("hex")}.codemod-tmp`;
      try {
        mkdirSync(dirname(absolute), { recursive: true });
        writeFileSync(temp, this.writes.get(path)!.content);
        if (lexists(absolute)) chmodSync(temp, statSync(absolute).mode);
        renameSync(temp, absolute);
      } catch (error) {
        rmSync(temp, { force: true });
        throw fail(
          path,
          [path, ...remainingAfter(index, "write")],
          (error as Error).message,
          false,
        );
      }
      applied.push(path);
    });
    deletes.forEach((path, index) => {
      if (signal?.aborted)
        throw fail(path, [path, ...remainingAfter(index, "delete")], "aborted", true);
      try {
        rmSync(resolveInsideRoot(this.root, path, "renamed file"), { force: true });
      } catch (error) {
        throw fail(
          path,
          [path, ...remainingAfter(index, "delete")],
          (error as Error).message,
          false,
        );
      }
      applied.push(path);
    });
    return { written: writes, deleted: deletes };
  }

  toJSON(): Json {
    return { writes: this.plan().writes, deletes: this.plan().deletes };
  }
}

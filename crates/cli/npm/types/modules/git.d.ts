// `codemod/git`: git steps for the workflow body. They only touch git when
// the run uses `--managed-git`; otherwise they are recorded as skipped.

declare module "codemod:git" {
  import type { Skippable, Task } from "codemod:workflow";

  /**
   * Checks out a branch for the following changes. Only with `--managed-git`.
   * Git steps share one checkout: one started while other steps are still
   * running fails, so run shards that use git one at a time.
   * @experimental
   */
  export function branch(name: string): Task<Skippable<{ branch: string }>>;

  /**
   * Commits changes. Only with `--managed-git`. Not alongside other steps; see `branch`.
   * @experimental
   */
  export function commit(
    message: string,
    options?: { add?: readonly string[]; allowEmpty?: boolean },
  ): Task<Skippable<{ committed: boolean }>>;

  /** @experimental */
  export interface PullRequestOptions {
    title: string;
    body?: string;
    draft?: boolean;
    base?: string;
    /** Defaults to the current branch. */
    branch?: string;
  }

  /**
   * Pushes the branch and opens a pull request. Only with `--managed-git`.
   * Not alongside other steps; see `branch`.
   * @experimental
   */
  export function pullRequest(
    options: PullRequestOptions,
  ): Task<Skippable<{ branch: string; url: string | null }>>;
}

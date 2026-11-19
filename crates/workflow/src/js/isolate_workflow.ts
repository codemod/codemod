// `codemod/workflow` inside steps and callbacks, for the files they import.
// The workflow file's own callbacks get `registry` and `semver` from
// `codemod-internal:dependencies` through the build; a helper file that
// imports them from `codemod/workflow` gets them here. The rest only work in
// the workflow body, so using one here says so; importing one still works,
// for helpers the body and callbacks share.

export { registry, semver } from "codemod-internal:dependencies";
export { AwaitingApprovalError, CanceledError, StepError } from "codemod-internal:orchestrator";

function bodyOnly(name: string): () => never {
  return () => {
    throw new Error(
      `${name}() can only be called in the workflow body, not inside a step or forEachFile callback`,
    );
  };
}

export const step = bodyOnly("step");
export const sleep = bodyOnly("sleep");
export const waitForEvent = bodyOnly("waitForEvent");
export const forEachFile = bodyOnly("forEachFile");
export const dependencyGraph = bodyOnly("dependencyGraph");
export const shell = bodyOnly("shell");
export const jssg = bodyOnly("jssg");
export const astGrep = bodyOnly("astGrep");
export const shard = bodyOnly("shard");
export const codemod = bodyOnly("codemod");
export const agent = bodyOnly("agent");
export const installSkill = bodyOnly("installSkill");
export const approval = bodyOnly("approval");
export const branch = bodyOnly("branch");
export const commit = bodyOnly("commit");
export const pullRequest = bodyOnly("pullRequest");

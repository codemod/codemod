// The `codemod/git` module of the engine's QuickJS context: `branch`,
// `commit` and `pullRequest`, the git steps. Inside steps and callbacks
// `codemod/git` is `isolate_git.ts`.

import { gitTasks } from "codemod-internal:orchestrator";

export const { branch, commit, pullRequest } = gitTasks;

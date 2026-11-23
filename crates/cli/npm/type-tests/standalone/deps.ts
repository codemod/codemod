import { dependencyGraph } from "codemod/deps";

export const task = dependencyGraph({}, (g) => g.workspaces().length);

/**
 * Three read-only analyses that do not depend on each other, run as one
 * parallel group. The result is a tuple in declaration order whatever order
 * the members finish in. How many actually overlap is the runtime's decision,
 * not the author's. Members of a group cannot feed each other; a group is for
 * independent work.
 *
 * The analyses are exported so `05-composed.ts` can reuse them.
 */
import { jssg, parallel } from "@codemod.com/orchestration";
import { countCalls, fileOf, unwrappedCalls } from "./lib/ast.ts";
import { Findings } from "./lib/schemas.ts";
import { sources } from "./lib/steps.ts";

/** Files still calling the deprecated API. */
export const legacyCalls = jssg({
  name: "legacy-calls",
  language: "typescript",
  ...sources,
  output: Findings,
  transform(root) {
    const calls = countCalls(root, "oldApi");
    return calls === 0 ? null : { output: { file: fileOf(root), calls } };
  },
});

/** Files already calling the new API. */
export const migratedCalls = jssg({
  name: "migrated-calls",
  language: "typescript",
  ...sources,
  output: Findings,
  transform(root) {
    const calls = countCalls(root, "newApi");
    return calls === 0 ? null : { output: { file: fileOf(root), calls } };
  },
});

/** Files calling the new API with the old argument shape (`newApi("x")`). */
export const unwrappedNewCalls = jssg({
  name: "unwrapped-new-calls",
  language: "typescript",
  ...sources,
  output: Findings,
  transform(root) {
    const calls = unwrappedCalls(root, "newApi").length;
    return calls === 0 ? null : { output: { file: fileOf(root), calls } };
  },
});

export default parallel(legacyCalls(), migratedCalls(), unwrappedNewCalls());

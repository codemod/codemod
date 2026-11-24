// `codemod:assess` in Node.js and Bun: the question builders, and `assess`,
// which asks through the native addon for the step the calling code runs in.
// The engine answers as it does for the CLI (crates/workflow/src/assess):
// the same context, providers and usage.

import { makeAssess } from "./assess-core.mts";
import { context } from "./context.mts";

export { choice, list, number, yesNo } from "./assess-core.mts";

export const assess = makeAssess(() => {
  const scope = context.getStore();
  const step = scope?.step;
  if (!scope || !step) return undefined;
  return (request: string) => scope.run.assess(step.position, request);
});

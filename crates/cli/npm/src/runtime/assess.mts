// `codemod:assess` in Node.js and Bun: the question builders, and `assess`,
// which asks through the native addon for the step the calling code runs in.
// The engine answers as it does for the CLI (crates/workflow/src/assess):
// the same context, providers and usage.

import * as core from "./assess-core.mts";
import { context, experimental } from "./context.mts";

function question<Args extends unknown[], Answer>(build: (...args: Args) => Answer) {
  return (...args: Args): Answer => {
    experimental("codemod/assess");
    return build(...args);
  };
}

export const choice: typeof core.choice = question(core.choice);
export const list: typeof core.list = question(core.list);
export const number = question(core.number);
export const yesNo = question(core.yesNo);

export const assess = core.makeAssess(() => {
  experimental("codemod/assess");
  const scope = context.getStore();
  const step = scope?.step;
  if (!scope || !step) return undefined;
  return (request: string) => scope.run.assess(step.position, request);
});

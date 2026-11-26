// `codemod/llm` is not available when Node.js runs the workflow yet.

import { experimental } from "./context.mts";

export function generate(): Promise<never> {
  experimental("codemod/llm");
  return Promise.reject(
    new Error(
      "codemod/llm is not available when Node.js runs the workflow; run it with the codemod CLI",
    ),
  );
}

export default generate;

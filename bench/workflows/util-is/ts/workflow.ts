// nodejs/util-is (registry 1.0.2) as a TS workflow: one jssg step, same transform file, same globs.
import { jssg } from "@codemod.com/orchestration";
import transform from "../pkg/src/workflow.ts";

export default jssg({
  name: "util-is",
  language: "typescript",
  include: ["**/*.js", "**/*.jsx", "**/*.mjs", "**/*.cjs", "**/*.cts", "**/*.mts", "**/*.ts", "**/*.tsx"],
  exclude: ["**/node_modules/**"],
  transform,
});

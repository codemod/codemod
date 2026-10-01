/**
 * One JSSG step, exported directly. `include`/`exclude` select the files; the
 * transform runs once per selected file in the QuickJS sandbox, receiving the
 * parsed program root. This one is read-only: it returns `output` without
 * `content`, and the step's result is every file's output in file order.
 *
 * The transform is bundled into its own artifact before this module runs, so
 * it may use its parameters, globals, and imported helpers (`lib/ast.ts`),
 * but nothing else declared in this file.
 */
import { jssg } from "@codemod.com/orchestration";
import { countCalls, fileOf } from "./lib/ast.ts";
import { Usages } from "./lib/schemas.ts";

export default jssg({
  name: "api-usage",
  language: "typescript",
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts", "**/*.generated.ts"],
  output: Usages,
  transform(root) {
    return {
      output: {
        file: fileOf(root),
        oldApi: countCalls(root, "oldApi"),
        newApi: countCalls(root, "newApi"),
      },
    };
  },
});

/**
 * Benchmark arms `ts-node`, `ts-quickjs`: the two benchmark
 * transforms as a sequence of JSSG steps for the TypeScript orchestration
 * runtime (`packages/orchestration`). Equivalent to ../yaml/workflow.yaml:
 * the same transform files (imported bindings, bundled into artifacts by the
 * build step), the same language, and the same include/exclude globs, so
 * both engines select the same files. No selector, no semantic analysis.
 *
 * `@codemod.com/orchestration` resolves through
 * `ts/node_modules/@codemod.com/orchestration`, a git-ignored symlink to
 * packages/orchestration that the harness (`lib.sh`) creates.
 */
import { jssg, sequence } from "@codemod.com/orchestration";
import consoleToLogger from "../transforms/console-to-logger.ts";
import renameVitestImport from "../transforms/rename-vitest-import.ts";

const files = {
  language: "typescript",
  include: ["**/*.{ts,mts,cts,js,mjs,cjs}"],
  exclude: ["**/node_modules/**"],
} as const;

const consoleStep = jssg({
  name: "console-to-logger",
  ...files,
  transform: consoleToLogger,
});

const importStep = jssg({
  name: "rename-vitest-import",
  ...files,
  transform: renameVitestImport,
});

export default sequence(consoleStep(), importStep());

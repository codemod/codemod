/**
 * Ordered steps with typed data flow. The deprecated `oldApi(x)` becomes
 * `newApi(x)` (a migration shared through `lib/steps.ts`), then the new API's
 * final shape `newApi({ name: x })` is applied to exactly the files the first
 * step changed, then a shell step verifies the result.
 *
 * Each stage's output is the next stage's input: `rename-calls` yields
 * `Migration[]`, which `wrap-options` declares as its `input` and reads as
 * `options.params.input`, and whose own `Migration[]` output builds the
 * `verify-migrations` command. The schemas validate the data at run time and
 * TypeScript checks the chain when the sequence is written: moving
 * `verify-migrations` between the two migrations is a type error, because
 * its `Verification` output is not the `Migration[]` that `wrap-options` needs.
 */
import { jssg, sequence } from "@codemod.com/orchestration";
import { fileOf, wrapCalls } from "./lib/ast.ts";
import { Migrations } from "./lib/schemas.ts";
import { renameCalls, verifyMigrations } from "./lib/steps.ts";

const wrapOptions = jssg({
  name: "wrap-options",
  language: "typescript",
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts", "**/*.generated.ts"],
  input: Migrations,
  output: Migrations,
  transform(root, options) {
    const file = fileOf(root);
    // The previous stage's output: only the files it migrated are touched.
    const migrated = options.params.input ?? [];
    if (!migrated.some((migration) => migration.file === file)) return null;
    const { content, replaced } = wrapCalls(root, "newApi");
    return { content, output: { file, replaced } };
  },
});

export default sequence(renameCalls(), wrapOptions(), verifyMigrations());

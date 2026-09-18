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
import { sequence } from "@codemod.com/orchestration";
import { renameCalls, verifyMigrations, wrapOptions } from "./lib/steps.ts";

export default sequence(renameCalls(), wrapOptions(), verifyMigrations());

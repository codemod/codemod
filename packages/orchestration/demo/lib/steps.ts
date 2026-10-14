/**
 * Steps several demo workflows share. A step is a description; creating one
 * runs nothing. The inline transforms here are bundled by `codemod-workflow`
 * exactly like the ones written in a workflow file, because the build step
 * splits every module in the import graph.
 */
import { jssg, shell } from "@codemod.com/orchestration";
import { fileOf, rewriteCalls, wrapCalls } from "./ast.ts";
import { Inventory, Migrations, Remaining, Verification } from "./schemas.ts";

/** The sources every step considers: `src`, minus declarations and generated code. */
export const sources = {
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts", "**/*.generated.ts"],
};

/** The same file set for shell steps, as `grep -r` options. */
const grepSources = "-r --include='*.ts' --exclude='*.d.ts' --exclude='*.generated.ts'";

/** Counts the matches of `pattern` in the sources; `0` when there are none. */
const countMatches = (pattern: string) =>
  `$(grep ${grepSources} -ho '${pattern}' src | wc -l | tr -d ' ')`;

/** Counts the source files containing `pattern`. */
const countFiles = (pattern: string) =>
  `$(grep ${grepSources} -l '${pattern}' src | wc -l | tr -d ' ')`;

/** How many source files still call the deprecated API. */
export const inspect = shell({
  name: "inspect",
  command: `printf '{"pending":%s}' "${countFiles("oldApi(")}"`,
  output: Inventory,
});

/**
 * The migration: `oldApi(x)` becomes `newApi(x)`. The static selector skips
 * files without a call before any sandbox starts, so the output lists exactly
 * the files that changed.
 */
export const renameCalls = jssg({
  name: "rename-calls",
  language: "typescript",
  ...sources,
  selector: { rule: { pattern: "oldApi($ARG)" } },
  output: Migrations,
  transform(root) {
    const { content, replaced } = rewriteCalls(root, "oldApi", "newApi");
    return { content, output: { file: fileOf(root), replaced } };
  },
});

/** Wraps the renamed calls in the options object required by the new API. */
export const wrapOptions = jssg({
  name: "wrap-options",
  language: "typescript",
  ...sources,
  input: Migrations,
  output: Migrations,
  transform(root, options) {
    const file = fileOf(root);
    const migrated = options.params.input ?? [];
    if (!migrated.some((migration) => migration.file === file)) return null;
    const { content, replaced } = wrapCalls(root, "newApi");
    return { content, output: { file, replaced } };
  },
});

/** Verification that needs no input: are any `oldApi(` call sites left? */
export const verifyNoLegacy = shell({
  name: "verify-no-legacy",
  command: `printf '{"remaining":%s}' "${countMatches("oldApi(")}"`,
  output: Remaining,
});

/**
 * Verification driven by the migration's own output: the number of files it
 * reports is part of the command, and the sources are checked for leftovers.
 */
export const verifyMigrations = shell({
  name: "verify-migrations",
  input: Migrations,
  output: Verification,
  command: (migrations) =>
    `printf '{"migrated":${migrations.length},"remaining":%s}' "${countMatches("oldApi(")}"`,
});

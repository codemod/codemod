/**
 * A workflow whose first stage requires input. The migration takes a `Config`
 * naming the replacement API; the CLI supplies it with `--input <json>` and
 * `run()` with `{ input }`. Without it the CLI refuses to start, and with the
 * wrong shape the schema rejects it before anything runs.
 *
 *   codemod-workflow demo/07-input.ts --target demo/target --input '{"replacement":"newApi"}'
 */
import { jssg, sequence } from "@codemod.com/orchestration";
import { fileOf, rewriteCalls } from "./lib/ast.ts";
import { Config, Migrations } from "./lib/schemas.ts";
import { verifyMigrations } from "./lib/steps.ts";

const migrateTo = jssg({
  name: "migrate-to",
  language: "typescript",
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts", "**/*.generated.ts"],
  selector: { rule: { pattern: "oldApi($ARG)" } },
  input: Config,
  output: Migrations,
  transform(root, options) {
    const config = options.params.input;
    if (config === undefined) throw new Error("migrate-to needs its Config input");
    const { content, replaced } = rewriteCalls(root, "oldApi", config.replacement);
    return { content, output: { file: fileOf(root), replaced } };
  },
});

export default sequence(migrateTo(), verifyMigrations());

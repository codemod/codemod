/**
 * The simplest workflow: one shell step, exported directly.
 *
 *   codemod-workflow demo/01-shell.ts --target demo/target
 *
 * runs the command in the target directory and prints its output. Without an
 * `output` schema a shell step yields `{ stdout }`; see `lib/steps.ts` for
 * steps whose stdout is parsed as JSON and validated.
 */
import { shell } from "@codemod.com/orchestration";

export default shell({
  name: "list-legacy-files",
  command: "grep -rl --include='*.ts' --exclude='*.d.ts' 'oldApi(' src | sort",
});

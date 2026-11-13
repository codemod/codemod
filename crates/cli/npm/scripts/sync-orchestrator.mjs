// Copies the workflow orchestrator from the engine into this package, where
// the Node.js host loads it. `--check` fails when the copy is out of date.

import { readFileSync, writeFileSync } from "node:fs";

const source = new URL("../../../workflow/src/js/orchestrator.js", import.meta.url);
const copy = new URL("../runtime/orchestrator.mjs", import.meta.url);

export const HEADER =
  "// Generated from crates/workflow/src/js/orchestrator.js by `pnpm sync-orchestrator`; do not edit.\n\n";

const expected = HEADER + readFileSync(source, "utf8");

if (process.argv.includes("--check")) {
  let actual = "";
  try {
    actual = readFileSync(copy, "utf8");
  } catch {
    // missing
  }
  if (actual !== expected) {
    process.exitCode = 1;
    process.stderr.write(
      "runtime/orchestrator.mjs is out of date; run `pnpm --filter codemod sync-orchestrator`\n",
    );
  }
} else {
  writeFileSync(copy, expected);
}

#!/usr/bin/env node
// Re-spawns Node with TypeScript support so `src/cli.ts` and the workflow it
// loads run without a build step. `register-ts.mjs` covers `.ts` files under
// `node_modules`, which Node's built-in type stripping refuses, so the bin
// also works from a workspace or package installation.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const cli = fileURLToPath(new URL("../src/cli.ts", import.meta.url));
const register = fileURLToPath(new URL("./register-ts.mjs", import.meta.url));
const result = spawnSync(
  process.execPath,
  [
    "--disable-warning=ExperimentalWarning",
    "--experimental-transform-types",
    "--import",
    register,
    cli,
    ...process.argv.slice(2),
  ],
  { stdio: "inherit" },
);
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;

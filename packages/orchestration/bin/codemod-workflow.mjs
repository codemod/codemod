#!/usr/bin/env node
// Runs `src/host/cli.ts` and the workflow it loads in this Node process, with
// no build step and no second Node: `register-ts.mjs` installs synchronous
// module hooks that strip types (transform mode, so enums and parameter
// properties work) from every `.ts` file, `node_modules` included, so the
// bin also works from a workspace or package installation.
//
// Startup is cached: stripped sources (`register-ts.mjs`) and V8's compiled
// code (`module.enableCompileCache`) live under the build cache
// (`$CODEMOD_WORKFLOW_CACHE` or `<tmpdir>/codemod-workflow-cache-<uid>`);
// `NODE_DISABLE_COMPILE_CACHE=1` turns the latter off.
//
// Needs Node >= 22.15 (`module.registerHooks`); the repository requires Node 24.
import { enableCompileCache } from "node:module";
import { privateDirectory } from "./register-ts.mjs";

const compiled = privateDirectory("v8");
if (compiled !== undefined) enableCompileCache(compiled);

const { main } = await import("../src/host/cli.ts");
await main(process.argv.slice(2));

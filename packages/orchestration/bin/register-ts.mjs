// Module hook that strips types from `.ts` / `.mts` / `.cts` files located
// under `node_modules`. Node's own type stripping handles every other `.ts`
// file but throws ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING for installed
// packages, which is exactly where this package lives once it is installed.
// Uses only Node built-ins (`module.registerHooks`, `module.stripTypeScriptTypes`).
import { readFileSync } from "node:fs";
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { fileURLToPath } from "node:url";

const TS_IN_NODE_MODULES = /^file:.*\/node_modules\/.*\.[cm]?ts$/u;

registerHooks({
  load(url, context, nextLoad) {
    if (!TS_IN_NODE_MODULES.test(url)) return nextLoad(url, context);
    const source = stripTypeScriptTypes(readFileSync(fileURLToPath(url), "utf8"), {
      mode: "transform",
      sourceUrl: url,
    });
    return {
      format: url.endsWith(".cts") ? "commonjs" : "module",
      source,
      shortCircuit: true,
    };
  },
});

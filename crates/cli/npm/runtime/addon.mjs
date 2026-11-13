// Loads the native addon (`codemod.node`): the workflow engine and the
// ast-grep API for code that runs in Node.js or Bun.
//
// Looked up in order: `CODEMOD_NODE_ADDON`, the platform package next to the
// CLI binary, then a cargo build in a checkout of the codemod repository.

import { existsSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));

let loaded;

/** The addon's exports. Throws with what to do when it cannot be found. */
export function addon() {
  if (loaded) return loaded;
  const path = addonPath();
  const module = { exports: {} };
  process.dlopen(module, path);
  loaded = module.exports;
  return loaded;
}

/** Where the addon is, or throws. */
export function addonPath() {
  if (process.env.CODEMOD_NODE_ADDON) return process.env.CODEMOD_NODE_ADDON;
  const { detectPackageName } = require("../codemod");
  const pkgName = detectPackageName();
  if (pkgName) {
    try {
      const candidate = join(
        dirname(require.resolve(`${pkgName}/package.json`, { paths: [here] })),
        "codemod.node",
      );
      if (existsSync(candidate)) return candidate;
    } catch {
      // not installed
    }
  }
  const library =
    process.platform === "win32"
      ? "codemod_node.dll"
      : process.platform === "darwin"
        ? "libcodemod_node.dylib"
        : "libcodemod_node.so";
  // In a checkout: whichever cargo build is newest, so a stale one is not
  // picked over the build just made.
  const builds = ["release", "debug"]
    .map((profile) => join(here, "..", "..", "..", "..", "target", profile, library))
    .filter((candidate) => existsSync(candidate))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  if (builds.length) return builds[0];
  throw new Error(
    `cannot find the codemod native addon${pkgName ? ` (expected codemod.node in the ${pkgName} package)` : " for this platform"}; reinstall codemod with optional dependencies, or set CODEMOD_NODE_ADDON`,
  );
}

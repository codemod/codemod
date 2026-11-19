// @codemod.com/jssg-types mirrors the type declarations that ship with the
// `codemod` package (crates/cli/npm/types), for projects that still import
// from this package. Run before packing; `src/` is generated, not committed.
import { cpSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const source = join(here, "..", "..", "..", "crates", "cli", "npm", "types");
const target = join(here, "..", "src");

rmSync(target, { recursive: true, force: true });
cpSync(source, target, {
  recursive: true,
  filter: (path) => !path.endsWith("tsconfig.json"),
});

// Declarations refer to their own package by name (`codemod/langs/tsx`).
const rename = (dir) => {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      rename(path);
    } else if (path.endsWith(".d.ts")) {
      const text = readFileSync(path, "utf8");
      const renamed = text.replace(/(["'])codemod\//g, "$1@codemod.com/jssg-types/");
      if (renamed !== text) writeFileSync(path, renamed);
    }
  }
};
rename(target);

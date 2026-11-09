import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { expect, it } from "vitest";

/** Each `src/` folder and the folders it may import from (README "Layout"). */
const ALLOWED: Record<string, readonly string[]> = {
  core: [],
  authoring: ["core"],
  execution: ["core"],
  runtime: ["core", "authoring", "execution"],
  host: ["core", "authoring", "execution", "runtime"],
};

const src = resolve(import.meta.dirname, "../src");
const layerOf = (file: string) => relative(src, file).split(/[\\/]/)[0]!;

it("src folders import only in the documented direction", () => {
  const files = readdirSync(src, { recursive: true, encoding: "utf8" })
    .filter((name) => name.endsWith(".ts"))
    .map((name) => join(src, name));
  const violations: string[] = [];
  for (const file of files) {
    // Entry points re-export across layers by design.
    if (file === join(src, "index.ts") || file === join(src, "portable.ts")) continue;
    const from = layerOf(file);
    expect(Object.keys(ALLOWED), relative(src, file)).toContain(from);
    const text = readFileSync(file, "utf8");
    for (const [, specifier] of text.matchAll(/(?:from\s+|import\(|new URL\()"(\.[^"]*)"/g)) {
      const to = layerOf(resolve(dirname(file), specifier!));
      if (to !== from && !ALLOWED[from]!.includes(to)) {
        violations.push(`${relative(src, file)} -> ${specifier} (${from} -> ${to})`);
      }
    }
  }
  expect(violations).toEqual([]);
});

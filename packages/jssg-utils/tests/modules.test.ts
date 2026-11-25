import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import { importedBindings, retargetImport } from "../src/javascript/exports/modules.ts";

function retarget(src: string, moves: Record<string, { from: string; name: string }>) {
  const root = parse("typescript", src);
  const statement = root.root().find({ rule: { kind: "import_statement" } })!;
  const edit = retargetImport(statement, (binding) => moves[binding.local] ?? null);
  return edit ? root.root().commitEdits([edit]) : null;
}

function equal(actual: unknown, expected: unknown, label: string) {
  assert(
    actual === expected,
    `${label}:\nexpected ${JSON.stringify(expected)}\ngot      ${JSON.stringify(actual)}`,
  );
}

// Named to default, one binding stays.
equal(
  retarget('import { Button, Modal } from "@/ui";', {
    Modal: { from: "@/ui/modal", name: "default" },
  }),
  'import { Button } from "@/ui";\nimport Modal from "@/ui/modal";',
  "named to default",
);

// Everything moves; renamed export keeps the local name; quotes and no semicolon kept.
equal(
  retarget("import Def, { a as b, c } from './barrel'", {
    Def: { from: "./def", name: "default" },
    b: { from: "./ab", name: "a" },
    c: { from: "./ab", name: "realC" },
  }),
  "import Def from './def'\nimport { a as b, realC as c } from './ab'",
  "all moved",
);

// Two defaults into one module; a moved type-only group becomes import type.
equal(
  retarget('import { type Props, X, Y } from "./lib";', {
    Props: { from: "./types", name: "Props" },
    X: { from: "./xy", name: "default" },
    Y: { from: "./xy", name: "default" },
  }),
  'import type { Props } from "./types";\nimport X, { default as Y } from "./xy";',
  "two defaults, inline type",
);

// Moved type-only bindings become an import type statement.
equal(
  retarget('import { type Config, load } from "./lib";', {
    Config: { from: "./lib/config", name: "Config" },
  }),
  'import { load } from "./lib";\nimport type { Config } from "./lib/config";',
  "moved inline types",
);

// import type stays import type.
equal(
  retarget('import type { A } from "./lib";', { A: { from: "./a", name: "A" } }),
  'import type { A } from "./a";',
  "import type",
);

// Nothing moves; namespace imports are left alone.
equal(retarget('import { A } from "./lib";', {}), null, "nothing moves");
equal(
  retarget('import * as ns from "./lib";', { ns: { from: "./x", name: "x" } }),
  null,
  "namespace",
);

const bindings = importedBindings(
  parse("typescript", 'import D, { a as b, type T } from "m";')
    .root()
    .find({ rule: { kind: "import_statement" } })!,
);
equal(
  JSON.stringify(bindings.map((b) => [b.local, b.imported, b.typeOnly])),
  JSON.stringify([
    ["D", "default", false],
    ["b", "a", false],
    ["T", "T", true],
  ]),
  "importedBindings",
);

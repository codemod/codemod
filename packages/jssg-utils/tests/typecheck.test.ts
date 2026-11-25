import { ok as assert } from "assert";
import {
  newTypeErrors,
  parseTypeErrors,
  typeErrorCauses,
} from "../src/javascript/exports/typecheck.ts";

function equal(actual: unknown, expected: unknown, label: string) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  assert(a === e, `${label}:\nexpected ${e}\ngot      ${a}`);
}

// Both tsc output formats; continuation lines are ignored.
const before = parseTypeErrors(
  [
    "src/legacy.ts(4,3): error TS2322: Type 'string' is not assignable to type 'number'.",
    "  Some continuation line.",
  ].join("\n"),
);
const after = parseTypeErrors(
  [
    "src/legacy.ts(9,3): error TS2322: Type 'string' is not assignable to type 'number'.",
    "src/App.ts:1:24 - error TS2307: Cannot find module './components' or its corresponding type declarations.",
    "src/pages/Home.ts(2,20): error TS2307: Cannot find module '../components/index.js' or its corresponding type declarations.",
    "src/moved/Card.ts(1,22): error TS2307: Cannot find module './theme' or its corresponding type declarations.",
    "src/App.ts(5,1): error TS2304: Cannot find name 'Buton'.",
  ].join("\n"),
);
equal(before.length, 1, "plain format");
equal(
  after[1],
  {
    file: "src/App.ts",
    line: 1,
    column: 24,
    code: "TS2307",
    message: "Cannot find module './components' or its corresponding type declarations.",
  },
  "pretty format",
);

// Errors that were there before do not count, even on another line.
const added = newTypeErrors(before, after);
equal(added.length, 4, "new errors");

// Each new error, by the change that caused it.
const barrel = { from: "src/components/index.ts", to: null };
const moved = { from: "src/Card.ts", to: "src/moved/Card.ts" };
const causes = typeErrorCauses(added, [barrel, moved]);
equal(
  causes.get(barrel)?.map((e) => e.file),
  ["src/App.ts", "src/pages/Home.ts"],
  "removed barrel",
);
equal(
  causes.get(moved)?.map((e) => e.file),
  ["src/moved/Card.ts"],
  "moved file",
);
equal(
  causes.get(null)?.map((e) => e.code),
  ["TS2304"],
  "not a file change",
);

// Aliases need a resolver.
const aliased = parseTypeErrors(
  "src/App.ts(1,20): error TS2307: Cannot find module '@/components'.",
);
equal(typeErrorCauses(aliased, [barrel]).get(null)?.length, 1, "alias without a resolver");
equal(
  typeErrorCauses(aliased, [barrel], (_from, spec) =>
    spec === "@/components" ? "src/components/index.ts" : null,
  ).get(barrel)?.length,
  1,
  "alias with a resolver",
);

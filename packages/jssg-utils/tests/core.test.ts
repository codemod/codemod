import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import { applyEdits, groupByFile, planEdit, planInsert } from "../src/core/exports/edits.ts";
import {
  enclosingClass,
  enclosingFunction,
  enclosingLoop,
  enclosingStatement,
} from "../src/core/exports/nodes.ts";

function equal(actual: unknown, expected: unknown, label: string) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  assert(a === e, `${label}:\nexpected ${e}\ngot      ${a}`);
}

const find = (lang: string, src: string, pattern: string) => {
  const node = parse(lang, src).root().find({ rule: { pattern } });
  assert(node, `no ${pattern}`);
  return node!;
};
const describe = (loop: ReturnType<typeof enclosingLoop>) =>
  loop && [loop.kind, loop.item?.text() ?? null, loop.iterable?.text() ?? null];

// --- enclosing* in TypeScript
const ts = `
export class Shop {
  load(ids: string[]) {
    for (const id of ids) { db.find(id); }
    ids.forEach((x) => db.get(x));
    ids.reduce((acc, y) => acc + db.count(y), 0);
    for (let i = 0; i < 3; i++) db.tick(i);
    for (const k in map) db.key(k);
    while (more()) db.next();
    for (const z of zs) { const f = () => db.later(z); }
    return db.all();
  }
}
export const top = make(1);
`;
const callIn = (pattern: string) => find("typescript", ts, pattern);
equal(enclosingFunction(callIn("db.find(id)"))?.kind(), "method_definition", "function: a method");
equal(enclosingClass(callIn("db.find(id)"))?.kind(), "class_declaration", "class");
equal(enclosingFunction(callIn("make(1)")), null, "function: module level");
equal(enclosingStatement(callIn("make(1)"))?.kind(), "export_statement", "statement: export");
equal(enclosingStatement(callIn("db.all()"))?.kind(), "return_statement", "statement: return");
equal(describe(enclosingLoop(callIn("db.find(id)"))), ["for-of", "id", "ids"], "loop: for-of");
equal(
  describe(enclosingLoop(callIn("db.get(x)"))),
  ["callback", "x", "ids"],
  "loop: forEach callback",
);
equal(
  describe(enclosingLoop(callIn("db.count(y)"))),
  ["callback", "y", "ids"],
  "loop: reduce element",
);
equal(describe(enclosingLoop(callIn("db.tick(i)"))), ["for", null, null], "loop: for(;;)");
equal(describe(enclosingLoop(callIn("db.key(k)"))), ["for-in", "k", "map"], "loop: for-in");
equal(describe(enclosingLoop(callIn("db.next()"))), ["while", null, null], "loop: while");
equal(
  enclosingLoop(callIn("db.later(z)")),
  null,
  "loop: a function declared in a loop does not loop",
);
equal(enclosingLoop(callIn("db.all()")), null, "loop: none");
equal(enclosingLoop(callIn("more()")), null, "loop: a while condition is not its body");

// --- enclosing* in Python
const py = `
class OrderView:
    def get(self, request):
        for order in orders:
            fetch(order.id)
        names = [lookup(o) for o in orders]
        return render(names)
`;
const pyCall = (pattern: string) => find("python", py, pattern);
equal(enclosingFunction(pyCall("fetch(order.id)"))?.kind(), "function_definition", "py function");
equal(enclosingClass(pyCall("fetch(order.id)"))?.kind(), "class_definition", "py class");
equal(describe(enclosingLoop(pyCall("fetch(order.id)"))), ["for-of", "order", "orders"], "py for");
equal(
  describe(enclosingLoop(pyCall("lookup(o)"))),
  ["comprehension", "o", "orders"],
  "py comprehension",
);
equal(enclosingStatement(pyCall("render(names)"))?.kind(), "return_statement", "py statement");

// --- edits across steps
const src = 'const é = "ünïcode"; call(a); other(b);\n';
const root = parse("typescript", src).root();
const a = root.find({ rule: { pattern: "call(a)" } })!;
const b = root.find({ rule: { pattern: "other(b)" } })!;
const plan = [planEdit(a, "await call(a)"), planEdit(b, "other(c)"), planInsert(a, "/* x */ ")];
equal(
  applyEdits(src, plan),
  'const é = "ünïcode"; /* x */ await call(a); other(c);\n',
  "non-ASCII before the edits; insert + replace at one start",
);
equal(
  applyEdits(src, [planEdit(a, "A"), { ...planEdit(a, "B"), end: planEdit(b, "").end }]),
  'const é = "ünïcode"; B;\n',
  "overlap: the outer edit wins",
);
let threw = false;
try {
  applyEdits(src, [planEdit(a, "A"), { ...planEdit(a, "B"), end: planEdit(b, "").end }], {
    overlap: "error",
  });
} catch {
  threw = true;
}
equal(threw, true, "overlap: error");
const grouped = groupByFile([
  { path: "a.ts", start: 0, end: 0, text: "1" },
  { path: "b.ts", start: 0, end: 0, text: "2" },
  { path: "a.ts", start: 1, end: 1, text: "3" },
]);
equal(
  [...grouped].map(([p, e]) => [p, e.length]),
  [
    ["a.ts", 2],
    ["b.ts", 1],
  ],
  "groupByFile",
);
equal(
  applyEdits(
    "xy",
    [
      { path: "a.ts", start: 1, end: 1, text: "!" },
      { path: "b.ts", start: 0, end: 0, text: "?" },
    ],
    { path: "a.ts" },
  ),
  "x!y",
  "path filter",
);

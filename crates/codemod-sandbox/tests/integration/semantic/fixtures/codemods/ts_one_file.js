// The symbols and dataflow lookups within one TypeScript file, in file
// scope: definitions and references, type-annotated names, and type
// positions.
import { crawl, dataflow, symbols } from "codemod:ast-grep";
import { equal, includes, test, throws } from "./spec.js";
export { run as default } from "./spec.js";

const declaredName = (root, name) =>
  root
    .root()
    .find({ rule: { kind: "variable_declarator", has: { field: "name", regex: `^${name}$` } } })
    .field("name");
const string = (root, text) => root.root().find({ rule: { kind: "string", regex: `^"${text}"$` } });
const texts = (edges) => (edges ?? []).map((e) => e.node.text());
// The text of every node a crawl reached.
const reachedTexts = (reached) => [...reached.keys()].map((node) => node.text());

test("definition of a use of a local variable lands on its name, as local", (root) => {
  const use = root
    .root()
    .find({ rule: { pattern: "x + 2" } })
    .field("left");
  const [def] = symbols("typescript").definition.get(use);
  equal([def.kind, def.node.equals(declaredName(root, "x"))], ["local", true]);
});

test("references of a variable are its uses", (root) => {
  const refs = symbols("typescript").references.get(declaredName(root, "x"));
  equal(texts(refs), ["x", "x"]);
});

test("references of a function are its calls", (root) => {
  const name = root
    .root()
    .find({ rule: { kind: "function_declaration" } })
    .field("name");
  equal(texts(symbols("typescript").references.get(name)), ["greet", "greet", "greet"]);
});

test("uses of a value assigned to a type-annotated name land on the name", async (root) => {
  const { uses } = dataflow("typescript");
  equal(
    uses.get(string(root, "z")).map((e) => e.node.kind() + " " + e.node.text()),
    ["identifier typedStr"],
  );
  includes(reachedTexts(await crawl(uses, string(root, "z"))), "typedStr");
});

test("a crawl from an array element reaches the type-annotated name", async (root) => {
  const { uses } = dataflow("typescript");
  equal(uses.get(string(root, "x"))[0].node.kind(), "array");
  includes(reachedTexts(await crawl(uses, string(root, "x"))), "typed");
});

test("a crawl starts from a type-annotated parameter", async (root) => {
  const echo = root
    .root()
    .find({ rule: { kind: "function_declaration", has: { field: "name", regex: "^echo$" } } });
  const param = echo.find({ rule: { kind: "required_parameter" } }).field("pattern");
  equal((await crawl(dataflow("typescript").uses, param)).has(param), true);
});

test("a type position has no uses, and a crawl from it reaches only itself", async (root) => {
  const { uses } = dataflow("typescript");
  const typeImport = root.root().find({ rule: { pattern: 'import("./input")' } });
  equal(uses.get(typeImport), undefined);
  equal((await crawl(uses, typeImport)).size, 1);
});

test("sources of a type alias declaration throw: there is no value there", async (root) => {
  const alias = root.root().find({ rule: { kind: "type_alias_declaration" } });
  await throws(
    () => dataflow("typescript").sources.get(alias),
    "sources.get(node) on `type_alias_declaration`: there is no value here",
  );
});

test("an expression next to a type position still has uses", (root) => {
  equal(
    dataflow("typescript").uses.get(root.root().find({ rule: { pattern: "f(1)" } }))?.length > 0,
    true,
  );
});

// Destructuring assignment targets and class fields: where values come from
// (a crawl of the sources lookup), what a field's initializer is stored in (the uses lookup), and
// which methods a method reaches (a crawl of the callees lookup).
import { callGraph, crawl, dataflow } from "codemod:ast-grep";

function describe(edges) {
  return edges.map((e) => e.kind + ":" + e.node.text() + (e.boundary ? "!" : "")).sort();
}

function expectHas(list, entry, label) {
  if (!list.includes(entry)) {
    throw new Error(label + ": expected " + entry + " in " + JSON.stringify(list));
  }
}

function expectLacks(list, entry, label) {
  if (list.includes(entry)) {
    throw new Error(label + ": did not expect " + entry + " in " + JSON.stringify(list));
  }
}

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default async function transform(root) {
  const { callees } = callGraph("javascript");
  const { sources, uses } = dataflow("javascript");
  // Every edge a crawl of the sources lookup reached (the start maps to null).
  const sourcesOf = async (node) => [...(await crawl(sources, node)).values()].filter((e) => e);
  const argOf = (pattern) => {
    const call = root.root().find({ rule: { pattern } });
    if (!call) throw new Error(pattern + " not found");
    return call
      .field("arguments")
      .children()
      .filter((n) => n.isNamed())[0];
  };
  const member = (kind, field, name) =>
    root
      .root()
      .find({ rule: { kind, has: { field, regex: "^" + name + "$" } } })
      .field(field);

  // [first, second = "none"] = pair(req.query.id)
  const second = describe(await sourcesOf(argOf("send(second)")));
  expectHas(second, "variable:second", "second");
  expectHas(second, "call:pair(req.query.id)!", "second");
  expectHas(second, 'literal:"none"', "second");
  expectHas(second, "parameter:req!", "second");

  // ({ name: rest } = obj)
  const rest = await sourcesOf(argOf("send(rest)"));
  const fromObj = rest.find((s) => s.kind === "parameter" && s.name === "obj");
  if (!fromObj) throw new Error("rest should come from obj: " + JSON.stringify(describe(rest)));

  // for ([first] of entries): first comes from entries and from pair(...)
  const first = describe(await sourcesOf(argOf("send(first)")));
  expectHas(first, "parameter:entries!", "first");
  expectHas(first, "call:pair(req.query.id)!", "first");

  // Each destructuring assignment rebinds first.
  const declared = member("variable_declarator", "name", "first");
  expectEqual(
    declared.mutations().map((m) => m.kind + " " + m.node.text()),
    [
      'reassign [first, second = "none"] = pair(req.query.id)',
      "reassign [first]",
      "escape send(first)",
    ],
    "mutations of first",
  );

  // Class fields: an arrow-function field is a method; `this.#advance()` and
  // `this.onTick()` resolve inside the class.
  const start = member("method_definition", "name", "start").parent();
  // A method's name, or the field's for an arrow-function field.
  const nameOf = (node) =>
    (node.field("name") ?? node.parent().field("property")).text();
  const reached = await crawl(callees, start);
  const depth = (node) => {
    let d = 0;
    for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) d++;
    return d;
  };
  expectEqual(
    [...reached]
      .filter(([, edge]) => edge)
      .map(([node, edge]) => nameOf(node) + ":" + edge.via + ":" + depth(node)),
    ["onTick:method:1", "#advance:method:2", "report:method:3"],
    "callees of start",
  );

  // A field initializer is stored in the field.
  const log = root.root().find({ rule: { kind: "array", inside: { kind: "field_definition" } } });
  const stored = (uses.get(log) ?? []).find((edge) => edge.via === "store");
  if (!stored || stored.name !== "log") {
    throw new Error("[] should be stored in field log: " + JSON.stringify(describe(uses.get(log) ?? [])));
  }

  // Can't tell: a computed method name is an unresolved call, not a guess,
  // so the callees lookup has no edge for it.
  const dispatch = member("method_definition", "name", "dispatch").parent();
  expectEqual(callees.get(dispatch), undefined, "callees of dispatch");

  // Can't tell: reading a field does not see its initializer yet (heap).
  // The trail ends at `this`, marked as a boundary.
  const count = describe(await sourcesOf(argOf("emit(this.count)")));
  expectHas(count, "this:this!", "this.count");
  expectLacks(count, "literal:0", "this.count");

  return null;
}

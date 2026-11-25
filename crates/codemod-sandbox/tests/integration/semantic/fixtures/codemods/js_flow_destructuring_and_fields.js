// Destructuring assignment targets and class fields.
function describe(steps) {
  return steps.map((s) => s.kind + ":" + s.node.text() + (s.boundary ? "!" : "")).sort();
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

export default function transform(root) {
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
  const second = describe(argOf("send(second)").flowsFrom());
  expectHas(second, "variable:second", "second");
  expectHas(second, "call:pair(req.query.id)!", "second");
  expectHas(second, 'literal:"none"', "second");
  expectHas(second, "parameter:req!", "second");

  // ({ name: rest } = obj)
  const rest = argOf("send(rest)").flowsFrom();
  const fromObj = rest.find((s) => s.kind === "parameter" && s.name === "obj");
  if (!fromObj) throw new Error("rest should come from obj: " + JSON.stringify(describe(rest)));

  // for ([first] of entries): first comes from entries and from pair(...)
  const first = describe(argOf("send(first)").flowsFrom());
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
  const start = member("method_definition", "name", "start");
  expectEqual(
    start
      .callees({ transitive: true })
      .filter((c) => c.resolved)
      .map((c) => c.name + ":" + c.via + ":" + c.depth),
    ["onTick:method:1", "#advance:method:2", "report:method:3"],
    "callees of start",
  );

  // A field initializer is stored in the field.
  const log = root.root().find({ rule: { kind: "array", inside: { kind: "field_definition" } } });
  const stored = log.flowsTo().find((s) => s.via === "store");
  if (!stored || stored.name !== "log") {
    throw new Error("[] should be stored in field log: " + JSON.stringify(describe(log.flowsTo())));
  }

  // Can't tell: a computed method name is an unresolved call, not a guess.
  const dispatch = member("method_definition", "name", "dispatch").callees();
  expectEqual(
    dispatch.map((c) => c.name + ":" + c.resolved),
    ["this[name]:false"],
    "callees of dispatch",
  );

  // Can't tell: reading a field does not see its initializer yet (heap).
  // The trail ends at `this`, marked as a boundary.
  const count = describe(argOf("emit(this.count)").flowsFrom());
  expectHas(count, "this:this!", "this.count");
  expectLacks(count, "literal:0", "this.count");

  return null;
}

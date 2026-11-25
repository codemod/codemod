// flowsFrom / flowsTo with { interprocedural: true }: parameters continue at
// the callers' arguments, call results at the callee's returns, and a value
// that enters a function through a call leaves through that same call.
function basename(path) {
  return path.split("/").pop();
}

function describe(steps) {
  return steps
    .map(
      (s) =>
        basename(s.root.filename()) + ":" + s.kind + ":" + s.node.text() + (s.boundary ? "!" : ""),
    )
    .sort();
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

export default function transform(root) {
  const find = (pattern) => {
    const node = root.root().find({ rule: { pattern } });
    if (!node) throw new Error(pattern + " not found");
    return node;
  };
  const argOf = (pattern) =>
    find(pattern)
      .field("arguments")
      .children()
      .filter((n) => n.isNamed())[0];
  const ip = { interprocedural: true };

  // log(other): other is normalize("literal"), which returns value.trim()
  // in lib.ts, where value is "literal", and only that call's argument.
  const other = describe(argOf("log($A)").flowsFrom(ip));
  expectHas(other, 'app.ts:call:normalize("literal")', "log(other)");
  expectHas(other, "lib.ts:call:value.trim()!", "log(other)");
  expectHas(other, "lib.ts:parameter:value: string", "log(other)");
  expectHas(other, 'app.ts:literal:"literal"', "log(other)");
  expectLacks(other, "app.ts:member:req.body.name", "log(other)");
  // Without it, the call is a boundary and lib.ts is not entered.
  const shallow = describe(argOf("log($A)").flowsFrom());
  expectHas(shallow, 'app.ts:call:normalize("literal")!', "log(other) intraprocedural");
  if (shallow.some((s) => s.startsWith("lib.ts:"))) {
    throw new Error("intraprocedural flowsFrom left the file: " + JSON.stringify(shallow));
  }

  // A parameter with callers is not a boundary: its sources are the
  // arguments at every call site.
  const x = describe(find("return x").child(1).flowsFrom(ip));
  expectHas(x, "app.ts:parameter:x: unknown", "x");
  expectHas(x, "app.ts:literal:5", "x");
  expectHas(x, "app.ts:member:req.body.name", "x");

  // req.body.name goes into normalize's value, back out to its own call
  // only, then into query's sql.
  const body = find("req.body.name").flowsTo(ip);
  const to = describe(body);
  expectHas(to, "lib.ts:parameter:value: string", "req.body.name");
  expectHas(to, "lib.ts:parameter:sql: string", "req.body.name");
  expectHas(to, "app.ts:variable:name", "req.body.name");
  expectLacks(to, "app.ts:variable:other", "req.body.name");
  // step.from crosses files: lib.ts's sql was reached from the argument
  // "select " + name in app.ts.
  const sql = body.find((s) => s.kind === "parameter" && s.name === "sql");
  if (!sql.from || basename(sql.from.root.filename()) !== "app.ts") {
    throw new Error("sql.from should be a step in app.ts");
  }
  return null;
}

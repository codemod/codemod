// Builtin method models: Promise then/catch and Array callbacks. Fixture:
// the debarrel review's dynamic-import probe (consumer.ts) plus models.ts.
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

function basename(path) {
  return path.split("/").pop();
}

export default function transform(root) {
  const file = basename(root.filename());
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

  if (file === "consumer.ts") {
    // import("./lib").then((m) => m.b + a): the module reaches m, then m.b.
    const forward = describe(find('import("./lib")').flowsTo());
    expectHas(forward, "parameter:m", "import() flows to");
    expectHas(forward, "member:m.b", "import() flows to");
    return null;
  }
  if (file !== "models.ts") throw new Error("unexpected target " + file);

  // filter keeps elements: o is an element of orders.
  expectHas(describe(argOf("audit($A)").flowsFrom()), "parameter:orders!", "audit(o.id)");

  // rate is what fetchRate() resolves to; sum is reduce's accumulator,
  // seeded with 0 and fed by each order.amount.
  const sent = describe(argOf("send($A)").flowsFrom());
  expectHas(sent, "call:fetchRate()!", "send(sum * rate)");
  expectHas(sent, "literal:0", "send(sum * rate)");
  expectHas(sent, "member:order.amount", "send(sum * rate)");

  // throughCalls: false stops at calls, models included.
  const shallow = describe(argOf("send($A)").flowsFrom({ throughCalls: false }));
  expectHas(shallow, "parameter:rate!", "shallow send");
  expectLacks(shallow, "call:fetchRate()!", "shallow send");
  expectLacks(shallow, "literal:0", "shallow send");

  // Can't tell: a rejection reason, and a named callback (its parameter
  // belongs to all its callers) stay boundaries.
  expectEqual(describe(argOf("report($A)").flowsFrom()), ["parameter:err!"], "report(err)");
  expectEqual(describe(argOf("record($A)").flowsFrom()), ["parameter:rate!"], "record(rate)");

  // Calls through a dynamic import's namespace resolve.
  for (const call of ["c()", "mod.c()"]) {
    expectEqual(
      find(call)
        .callees()
        .map((c) => c.name + "@" + basename(c.root.filename()) + ":" + c.resolved),
      ["c@c.ts:true"],
      call,
    );
  }
  return null;
}

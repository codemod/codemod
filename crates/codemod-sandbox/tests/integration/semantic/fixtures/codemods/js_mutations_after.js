// "Is this variable mutated after Y?"
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function describe(mutations) {
  return mutations.map(
    (m) =>
      m.kind +
      ":" +
      m.confidence +
      (m.viaAlias ? ":alias" : "") +
      (m.deferred ? ":deferred" : "") +
      " " +
      m.node.text().split("\n")[0],
  );
}

export default function transform(root) {
  const declared = (name) =>
    root
      .root()
      .find({
        rule: { kind: "variable_declarator", has: { field: "name", regex: "^" + name + "$" } },
      })
      .field("name");
  const audit = root.root().find({ rule: { pattern: "audit(total)" } });

  const items = declared("items");
  expectEqual(
    describe(items.mutations()),
    [
      'method-call:likely items.push({ sku: "gift" })',
      "escape:maybe sum(items)",
      "method-call:likely:alias snapshot.sort(byPrice)",
      "property-write:definite:deferred items.length = 0",
    ],
    "all mutations of items",
  );
  // After audit(total): the sort through the `snapshot` alias, and the write
  // in the timer callback, which can run at any time.
  expectEqual(
    describe(items.mutations({ after: audit })),
    [
      "method-call:likely:alias snapshot.sort(byPrice)",
      "property-write:definite:deferred items.length = 0",
    ],
    "mutations of items after audit(total)",
  );

  expectEqual(
    describe(declared("count").mutations({ after: audit })),
    ["reassign:definite count = count + coupons.length"],
    "mutations of count after audit(total)",
  );

  // `total` is handed to audit() at that point, and never touched after it.
  expectEqual(
    describe(declared("total").mutations({ after: audit })),
    [],
    "mutations of total after audit(total)",
  );

  return null;
}

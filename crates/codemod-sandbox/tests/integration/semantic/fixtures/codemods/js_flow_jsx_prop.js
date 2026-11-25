// "What flows into this JSX prop?"
function describe(steps) {
  return steps.map((s) => s.kind + ":" + s.node.text() + (s.boundary ? "!" : "")).sort();
}

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const attribute = (name) =>
    root
      .root()
      .findAll({ rule: { kind: "jsx_attribute" } })
      .find((a) => a.text().startsWith(name + "="));

  // title={title}: the result of formatName(user.first, user.last), built
  // from the `user` prop.
  const title = attribute("title").flowsFrom();
  expectEqual(
    describe(title.filter((s) => s.boundary || s.kind === "member")),
    [
      "call:formatName(user.first, user.last)!",
      "member:user.first",
      "member:user.last",
      "parameter:user!",
    ],
    "title",
  );

  // className={cls}: one of two string literals. The `theme` prop picks
  // between them but its value does not flow into the attribute.
  const className = attribute("className").flowsFrom();
  expectEqual(
    describe(className.filter((s) => s.kind === "literal" || s.kind === "parameter")),
    ['literal:"card dark"', 'literal:"card"'],
    "className",
  );

  // The other direction: where the `user` prop ends up. onClose closes over
  // `user` but is a new function, not a value computed from it.
  const user = root
    .root()
    .find({ rule: { kind: "shorthand_property_identifier_pattern", regex: "^user$" } });
  const attributes = user
    .flowsTo()
    .filter((s) => s.kind === "jsx-attribute")
    .map((s) => s.name)
    .sort();
  expectEqual(attributes, ["title"], "attributes fed by user");

  return null;
}

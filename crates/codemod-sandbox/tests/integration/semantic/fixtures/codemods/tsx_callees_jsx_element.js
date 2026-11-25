// callees() on a JSX element is the component it renders. Fixture: the i18n
// review's visibility case (Toolbar.tsx), plus member tags (Nav.tsx).
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

function describe(entries) {
  return entries.map(
    (c) =>
      (c.resolved ? c.name + "@" + basename(c.root.filename()) : "u:" + c.name) +
      ":" +
      c.via +
      ":" +
      c.depth,
  );
}

function element(root, name) {
  const found = root.root().find({
    rule: {
      any: [{ kind: "jsx_element" }, { kind: "jsx_self_closing_element" }],
      regex: "^<" + name.replace(".", "\\.") + "[\\s>/]",
    },
  });
  if (!found) throw new Error("<" + name + "> not found");
  return found;
}

export default function transform(root) {
  const file = basename(root.filename());

  if (file === "Toolbar.tsx") {
    const tooltip = element(root, "Tooltip");
    const opening = tooltip.field("open_tag") || tooltip.child(0);
    const name = opening.field("name");
    // The element, its opening tag and its name all mean the same call.
    for (const [label, node] of [
      ["element", tooltip],
      ["opening", opening],
      ["name", name],
    ]) {
      expectEqual(describe(node.callees()), ["Tooltip@Tooltip.tsx:jsx:1"], label);
    }
    expectEqual(
      describe(element(root, "TrackedButton").callees()),
      ["TrackedButton@TrackedButton.tsx:jsx:1"],
      "TrackedButton",
    );

    // An intrinsic element is not a call: its callees are the calls written
    // inside it, not every call of the enclosing function (console.log,
    // analytics.track are outside the <div>).
    expectEqual(
      describe(element(root, "div").callees()),
      [
        "TrackedButton@TrackedButton.tsx:jsx:1",
        "Tooltip@Tooltip.tsx:jsx:1",
        "statusLabel@status.ts:direct:1",
      ],
      "div",
    );
  } else if (file === "Nav.tsx") {
    expectEqual(
      describe(element(root, "Menu.Item").callees()),
      ["Item@Menu.tsx:jsx:1"],
      "Menu.Item",
    );
    // Can't tell: a member tag on something unknown is reported unresolved.
    expectEqual(
      describe(element(root, "props.Slot").callees()),
      ["u:props.Slot:jsx:1"],
      "props.Slot",
    );
  } else {
    throw new Error("unexpected target " + file);
  }
  return null;
}

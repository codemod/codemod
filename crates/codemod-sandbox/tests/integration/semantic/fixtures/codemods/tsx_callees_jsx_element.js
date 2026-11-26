// The callees lookup on a JSX element answers with the component it renders.
// Fixture: the i18n review's visibility case (Toolbar.tsx), plus member tags
// (Nav.tsx).
import { callGraph } from "codemod:ast-grep";

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

const { callees } = callGraph("tsx");

function describe(node) {
  return (callees.get(node) ?? []).map(
    (e) => e.node.field("name").text() + "@" + basename(e.node.getRoot().filename()) + ":" + e.via,
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
      expectEqual(describe(node), ["Tooltip@Tooltip.tsx:jsx"], label);
    }
    expectEqual(
      describe(element(root, "TrackedButton")),
      ["TrackedButton@TrackedButton.tsx:jsx"],
      "TrackedButton",
    );

    // An intrinsic element is not a call: its callees are the calls written
    // inside it, not every call of the enclosing function (console.log,
    // analytics.track are outside the <div>).
    expectEqual(
      describe(element(root, "div")),
      [
        "TrackedButton@TrackedButton.tsx:jsx",
        "Tooltip@Tooltip.tsx:jsx",
        "statusLabel@status.ts:direct",
      ],
      "div",
    );
  } else if (file === "Nav.tsx") {
    expectEqual(
      describe(element(root, "Menu.Item")),
      ["Item@Menu.tsx:jsx"],
      "Menu.Item",
    );
    // Can't tell: a member tag on something unknown has no edge.
    expectEqual(describe(element(root, "props.Slot")), [], "props.Slot");
  } else {
    throw new Error("unexpected target " + file);
  }
  return null;
}

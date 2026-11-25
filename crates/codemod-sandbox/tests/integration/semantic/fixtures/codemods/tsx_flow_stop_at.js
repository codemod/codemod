// flowsTo({ stopAt }): the first sink, not every element around it. Fixture:
// the i18n review's visibility case. A prop string used to report jsx-child
// steps for every ancestor element, so "is any step a jsx-child?" said
// every prop is visible text.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function expectThrow(fn, needle, label) {
  try {
    fn();
  } catch (error) {
    if (String(error).includes(needle)) return;
    throw new Error(label + ": wrong error " + error);
  }
  throw new Error(label + ": expected a throw");
}

export default function transform(root) {
  const literal = (text) => {
    const found = root.root().find({ rule: { kind: "string", regex: '^"' + text + '"$' } });
    if (!found) throw new Error(text + " not found");
    return found;
  };
  const vias = (steps) => steps.map((s) => s.via + ":" + s.depth);

  const tooltip = literal("Saves all open documents");
  const all = tooltip.flowsTo();
  if (!all.some((s) => s.via === "jsx-child")) {
    throw new Error("without stopAt the walk goes on up the tree: " + JSON.stringify(vias(all)));
  }
  const sinks = ["jsx-child", "jsx-attribute", "argument"];
  expectEqual(vias(tooltip.flowsTo({ stopAt: sinks })), ["jsx-attribute:1"], "Tooltip content");

  // A real child string stops at its element.
  expectEqual(
    vias(literal("Items").flowsTo({ stopAt: sinks })),
    ["alias:1", "alias:2", "jsx-child:3"],
    '{isBulk ? "Items" : "Item"}',
  );

  expectThrow(
    () => tooltip.flowsTo({ stopAt: ["jsx-kid"] }),
    "unknown edge kind in stopAt",
    "typo",
  );
  expectThrow(() => tooltip.flowsTo({ stopAt: "jsx-child" }), "must be an array", "not an array");
  return null;
}

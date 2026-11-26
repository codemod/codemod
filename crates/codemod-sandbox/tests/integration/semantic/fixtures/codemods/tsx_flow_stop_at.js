// crawl(uses, start, { stopAt }): the first sink, not every element around
// it. Fixture: the i18n review's visibility case. A prop string used to
// report jsx-child steps for every ancestor element, so "is any step a
// jsx-child?" said every prop is visible text. stopAt takes nodes, so the
// sinks are the nodes a first crawl reaches through a sink edge.
import { crawl, dataflow } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

async function expectThrow(fn, needle, label) {
  try {
    await fn();
  } catch (error) {
    if (String(error).includes(needle)) return;
    throw new Error(label + ": wrong error " + error);
  }
  throw new Error(label + ": expected a throw");
}

const SINKS = ["jsx-child", "jsx-attribute", "argument"];

export default async function transform(root) {
  const { uses } = dataflow("tsx");
  const literal = (text) => {
    const found = root.root().find({ rule: { kind: "string", regex: '^"' + text + '"$' } });
    if (!found) throw new Error(text + " not found");
    return found;
  };
  // Each reached node's edge and its distance from the start, in crawl order.
  const depth = (reached, node) => {
    let d = 0;
    for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) d++;
    return d;
  };
  const vias = (reached) =>
    [...reached].filter(([, edge]) => edge).map(([node, edge]) => edge.via + ":" + depth(reached, node));
  const toFirstSink = async (start) => {
    const all = await crawl(uses, start);
    const sinks = [...all.values()].filter((e) => e && SINKS.includes(e.via)).map((e) => e.node);
    return crawl(uses, start, { stopAt: sinks });
  };

  const tooltip = literal("Saves all open documents");
  const all = await crawl(uses, tooltip);
  if (![...all.values()].some((e) => e?.via === "jsx-child")) {
    throw new Error("without stopAt the crawl goes on up the tree: " + JSON.stringify(vias(all)));
  }
  expectEqual(vias(await toFirstSink(tooltip)), ["jsx-attribute:1"], "Tooltip content");

  // A real child string stops at its element.
  expectEqual(
    vias(await toFirstSink(literal("Items"))),
    ["alias:1", "alias:2", "jsx-child:3"],
    '{isBulk ? "Items" : "Item"}',
  );

  // stopAt holds nodes, not edge kinds.
  await expectThrow(
    () => crawl(uses, tooltip, { stopAt: ["jsx-kid"] }),
    "is not a node or a node.key()",
    "an edge kind",
  );
  await expectThrow(() => crawl(uses, tooltip, { stopAt: "jsx-child" }), "must be iterable", "not an array");
  return null;
}

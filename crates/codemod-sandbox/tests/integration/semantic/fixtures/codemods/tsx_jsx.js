// JSX: the callees lookup on an element answers with the component it
// renders, and a crawl of uses stops at the first sink. Fixture: the i18n
// review's visibility case (Toolbar.tsx), plus member tags (Nav.tsx).
import { callGraph, crawl, dataflow } from "codemod:ast-grep";
import { equal, onFile, test, throws } from "./spec.js";
export { run as default } from "./spec.js";

const callees = (node) =>
  (callGraph("tsx").callees.get(node) ?? []).map(
    (e) =>
      e.node.field("name").text() +
      "@" +
      e.node.getRoot().filename().split("/").pop() +
      ":" +
      e.via,
  );
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

onFile("src/Toolbar.tsx", () => {
  test("an element, its opening tag and its name all call the component", (root) => {
    const tooltip = element(root, "Tooltip");
    const opening = tooltip.field("open_tag") || tooltip.child(0);
    for (const node of [tooltip, opening, opening.field("name")]) {
      equal(callees(node), ["Tooltip@Tooltip.tsx:jsx"], node.kind());
    }
  });

  test("a self-closing element calls its component", (root) => {
    equal(callees(element(root, "TrackedButton")), ["TrackedButton@TrackedButton.tsx:jsx"]);
  });

  test("an intrinsic element's callees are the calls written inside it", (root) => {
    equal(callees(element(root, "div")), [
      "TrackedButton@TrackedButton.tsx:jsx",
      "Tooltip@Tooltip.tsx:jsx",
      "statusLabel@status.ts:direct",
    ]);
  });

  const SINKS = ["jsx-child", "jsx-attribute", "argument"];
  const literal = (root, text) =>
    root.root().find({ rule: { kind: "string", regex: '^"' + text + '"$' } });
  const depth = (reached, node) => {
    let d = 0;
    for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) d++;
    return d;
  };
  const vias = (reached) =>
    [...reached]
      .filter(([, edge]) => edge)
      .map(([node, edge]) => edge.via + ":" + depth(reached, node));
  // A crawl of uses that stops at the nodes the first crawl reached through a sink edge.
  const toFirstSink = async (start) => {
    const { uses } = dataflow("tsx");
    const all = await crawl(uses, start);
    const sinks = [...all.values()].filter((e) => e && SINKS.includes(e.via)).map((e) => e.node);
    return crawl(uses, start, { stopAt: sinks });
  };

  test("a crawl of uses without stopAt goes on up the element tree", async (root) => {
    const all = await crawl(dataflow("tsx").uses, literal(root, "Saves all open documents"));
    equal(
      [...all.values()].some((e) => e?.via === "jsx-child"),
      true,
      vias(all).join(),
    );
  });

  test("stopAt the first sink ends a prop string at its attribute", async (root) => {
    equal(vias(await toFirstSink(literal(root, "Saves all open documents"))), ["jsx-attribute:1"]);
  });

  test("stopAt the first sink ends a child string at its element", async (root) => {
    equal(vias(await toFirstSink(literal(root, "Items"))), ["alias:1", "alias:2", "jsx-child:3"]);
  });

  test("stopAt takes nodes, not edge kinds", async (root) => {
    const start = literal(root, "Saves all open documents");
    await throws(
      () => crawl(dataflow("tsx").uses, start, { stopAt: ["jsx-kid"] }),
      "is not a node or a node.key()",
    );
    await throws(
      () => crawl(dataflow("tsx").uses, start, { stopAt: "jsx-child" }),
      "must be iterable",
    );
  });
});

onFile("src/Nav.tsx", () => {
  test("a member tag calls the member component", (root) => {
    equal(callees(element(root, "Menu.Item")), ["Item@Menu.tsx:jsx"]);
  });

  test("a member tag on something unknown has no callee", (root) => {
    equal(callees(element(root, "props.Slot")), []);
  });
});

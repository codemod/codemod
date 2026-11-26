// "What flows into this JSX prop?" (a crawl of the sources lookup), and the
// other direction: which JSX attributes a crawl of the uses lookup reaches
// from a prop.
import { crawl, dataflow } from "codemod:ast-grep";

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

export default async function transform(root) {
  const { sources, uses } = dataflow("javascript");
  // Every edge a crawl of the sources lookup reached (the start maps to null).
  const sourcesOf = async (node) => [...(await crawl(sources, node)).values()].filter((e) => e);
  const attribute = (name) =>
    root
      .root()
      .findAll({ rule: { kind: "jsx_attribute" } })
      .find((a) => a.text().startsWith(name + "="));

  // title={title}: the result of formatName(user.first, user.last), built
  // from the `user` prop.
  const title = await sourcesOf(attribute("title"));
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
  const className = await sourcesOf(attribute("className"));
  expectEqual(
    describe(className.filter((s) => s.kind === "literal" || s.kind === "parameter")),
    ['literal:"card dark"', 'literal:"card"'],
    "className",
  );

  // The other direction: where the `user` prop ends up, crawling the uses
  // lookup. onClose closes over `user` but is a new function, not a value
  // computed from it: close(user.id) reaches the arrow function only as the
  // value it returns. A returned value leaves its function, so the crawl
  // stops at what a return (or a store into the heap) reaches.
  const user = root
    .root()
    .find({ rule: { kind: "shorthand_property_identifier_pattern", regex: "^user$" } });
  const all = await crawl(uses, user);
  const leaves = [...all.values()]
    .filter((edge) => edge?.via === "return" || edge?.via === "store")
    .map((edge) => edge.node);
  const reached = await crawl(uses, user, { stopAt: leaves });
  const attributes = [...reached.values()]
    .filter((edge) => edge?.kind === "jsx-attribute")
    .map((edge) => edge.name)
    .sort();
  expectEqual(attributes, ["title"], "attributes fed by user");

  return null;
}

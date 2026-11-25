// crawl() walks a lookup from a start node, and says how each node was reached.
import { callGraph, crawl } from "codemod:ast-grep";

const name = (n) => n.text().match(/function (\w+)/)[1];

export default async function transform(root) {
  const { callees } = callGraph("typescript");
  const start = root.root().find("function Profile() { $$$ }");
  const reached = await crawl(callees, start);
  const out = [...reached].map(([node, edge]) => name(node) + " <- " + (edge ? name(edge.from) : "start"));
  if (JSON.stringify(out) !== JSON.stringify(["Profile <- start", "format <- Profile"])) {
    throw new Error(JSON.stringify(out));
  }
}

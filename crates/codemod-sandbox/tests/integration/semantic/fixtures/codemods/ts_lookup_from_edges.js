// lookup(edges) keys edges by syntax node, so a node from another query finds them.
import { lookup } from "codemod:ast-grep";

export default function transform(root) {
  const a = root.root().find("function Profile() { $$$ }");
  const b = root.root().find("function format($$$) { $$$ }");
  const renders = lookup([[a, b, { why: "renders" }]]);
  const again = root.root().find("function Profile() { $$$ }");
  const [edge] = renders.get(again);
  if (edge.why !== "renders" || edge.node.key() !== b.key()) throw new Error(JSON.stringify(edge));
  if (renders.size !== 1 || [...renders.keys()][0].key() !== a.key()) throw new Error("listing");
}

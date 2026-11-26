// A value assigned to a type-annotated variable lands on the variable's name,
// so crawl can keep walking from it.
import { dataflow, crawl } from "codemod:ast-grep";

export default async function transform(root) {
  const { uses } = dataflow("typescript");
  const lit = (text) => root.root().find({ rule: { kind: "string", regex: `^"${text}"$` } });
  for (const text of ["x", "z"]) {
    const [edge] = uses.get(lit(text)) ?? [];
    // "x" goes into its array first; "z" straight into the variable.
    const expected = text === "x" ? "array" : "identifier";
    if (!edge || edge.node.kind() !== expected) {
      throw new Error(`${text}: ${edge ? edge.node.kind() + " " + edge.node.text() : "no edge"}`);
    }
    const reached = await crawl(uses, lit(text));
    const names = [...reached.keys()].map((n) => n.text());
    if (!names.includes(text === "x" ? "typed" : "typedStr")) {
      throw new Error(`${text}: ${JSON.stringify(names)}`);
    }
  }
  const param = root.root().find({ rule: { kind: "required_parameter" } }).find({ rule: { kind: "identifier" } });
  await crawl(uses, param);
}

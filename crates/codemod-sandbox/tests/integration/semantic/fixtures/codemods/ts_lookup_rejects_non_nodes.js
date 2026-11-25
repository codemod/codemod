// lookup(edges) takes engine nodes only, and says which edge is wrong.
import { lookup } from "codemod:ast-grep";

export default function transform(root) {
  const a = root.root().find("function Profile() { $$$ }");
  try {
    lookup([[a, a], [a, "format"]]);
  } catch (e) {
    if (e instanceof TypeError && e.message.includes("edge 1")) return;
    throw e;
  }
  throw new Error("a string target must throw");
}

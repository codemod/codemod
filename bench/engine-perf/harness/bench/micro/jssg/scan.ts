import type { Codemod } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
// Same work as the forEachFile callback: parse, count call expressions, change nothing.
const scan: Codemod<TSX> = async (root) => {
  root.root().findAll({ rule: { kind: "call_expression" } }).length;
  return null;
};
export default scan;

import type { Transform } from "codemod/ast-grep";
import type Json from "codemod/ast-grep/langs/json";

const transform: Transform<Json> = async (root) => {
  return root.root().text().includes('"chalk"') ? "{}" : null;
};

export default transform;

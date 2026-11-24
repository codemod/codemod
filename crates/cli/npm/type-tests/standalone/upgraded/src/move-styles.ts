import type { SgRoot } from "codemod/ast-grep";

export default function transform(root: SgRoot): null {
  root.rename(root.filename().replace(/\.less$/, ".css"));
  return null;
}

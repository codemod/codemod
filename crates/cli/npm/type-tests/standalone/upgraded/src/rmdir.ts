import type { SgRoot } from "codemod/ast-grep";
import type JS from "codemod/ast-grep/langs/javascript";

export default function transform(root: SgRoot<JS>): string | null {
  const calls = root.root().findAll({ rule: { pattern: "fs.rmdirSync($PATH)" } });
  if (!calls.length) return null;
  return root.root().commitEdits(calls.map((call) => call.replace("fs.rmSync()")));
}

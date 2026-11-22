// An imported forEachFile callback that type-tests/workflow.ts uses.

import { parse } from "codemod/ast-grep";
import type { FileCallback } from "codemod/workflow";

const migrate: FileCallback<{ path: string; count: number }> = (file) => {
  const root = parse(file).root();
  const found = root.findAll({ rule: { pattern: "var $A = $B" } });
  if (!found.length) return;
  file.write(
    root.commitEdits(found.map((node) => node.replace(node.text().replace(/^var/, "let")))),
  );
  return { path: file.path, count: found.length };
};

export default migrate;

// A JSSG transform that type-tests/workflow.ts imports.

import type { Codemod } from "codemod/ast-grep";
import type TS from "codemod/ast-grep/langs/typescript";

const codemod: Codemod<TS> = async (root) => {
  const edits = root
    .root()
    .findAll({ rule: { pattern: "var $X = $Y" } })
    .map((node) =>
      node.replace(`let ${node.getMatch("X")?.text()} = ${node.getMatch("Y")?.text()}`),
    );
  return edits.length ? root.root().commitEdits(edits) : null;
};

export default codemod;

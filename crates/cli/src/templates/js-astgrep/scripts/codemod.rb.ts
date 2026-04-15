import type { Codemod } from "codemod:ast-grep";
import type Ruby from "codemod:ast-grep/langs/ruby";

const codemod: Codemod<Ruby> = async (root) => {
  const rootNode = root.root();

  const nodes = rootNode.findAll({
    rule: {
      pattern: "puts $ARGS",
    },
  });

  const edits = nodes.map(node => {
    const args = node.getMatch("ARGS")?.text();
    return node.replace(`logger.info(${args})`);
  });

  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default codemod;

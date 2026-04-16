import type { Codemod } from "codemod:ast-grep";
import type C from "codemod:ast-grep/langs/c";

const codemod: Codemod<C> = async (root) => {
  const rootNode = root.root();

  const nodes = rootNode.findAll({
    rule: {
      any: [
        { pattern: "console.log($ARG)" },
        { pattern: "console.debug($ARG)" },
      ]
    },
  });

  const edits = nodes.map(node => {
    const arg = node.getMatch("ARG")?.text();
    return node.replace(`logger.log(${arg})`);
  });

  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default codemod;

import type { Codemod } from "codemod:ast-grep";
import type CSharp from "codemod:ast-grep/langs/c_sharp";

const codemod: Codemod<CSharp> = async (root) => {
  const rootNode = root.root();

  const nodes = rootNode.findAll({
    rule: {
      any: [
        { pattern: "Console.WriteLine($ARG)" },
        { pattern: "Console.Write($ARG)" },
      ]
    },
  });

  const edits = nodes.map(node => {
    const arg = node.getMatch("ARG")?.text();
    return node.replace(`Logger.Log(${arg})`);
  });

  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default codemod;

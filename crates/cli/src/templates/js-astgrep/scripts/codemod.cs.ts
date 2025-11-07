import type { SgRoot } from "codemod:ast-grep";
import type CSharp from "codemod:ast-grep/langs/c_sharp";

async function transform(root: SgRoot<CSharp>): Promise<string> {
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

export default transform;

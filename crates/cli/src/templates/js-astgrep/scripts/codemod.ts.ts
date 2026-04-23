import type { Codemod } from "codemod:ast-grep";
import type JS from "codemod:ast-grep/langs/javascript";
import type TS from "codemod:ast-grep/langs/typescript";
import type TSX from "codemod:ast-grep/langs/tsx";

// You can change the language to JS, TS, or TSX depending on your needs. Here we use a union type to support all three.
// Please note that TSX is different from TS in that it supports JSX syntax and treats type generics differently, so make sure to choose the one that best fits your codebase.
// - If you are targeting JSX files, use TSX.
// - If you are targeting plain TypeScript files without JSX, use TS.
// - If you do not care about TypeScript features and want to target plain JavaScript files, use JS.
//
// Make sure this is in sync with workflow.yaml where you specify the language for the codemod.
type JSOrTS = JS | TS | TSX;

const codemod: Codemod<JSOrTS> = async (root) => {
  const rootNode = root.root();

  const nodes = rootNode.findAll({
    rule: {
      pattern: "var $VAR = $VALUE",
    },
  });

  const edits = nodes.map((node) => {
    const varName = node.getMatch("VAR")?.text();
    const value = node.getMatch("VALUE")?.text();
    return node.replace(`const ${varName} = ${value}`);
  });

  const newSource = rootNode.commitEdits(edits);
  return newSource;
};

export default codemod;

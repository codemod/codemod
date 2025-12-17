import type { Transform } from "codemod:ast-grep";
import type Python from "codemod:ast-grep/langs/python";

const transform: Transform<Python> = async (root) => {
  const rootNode = root.root();

  // Find all Python 2-style exception handlers (using comma instead of 'as')
  const nodes = rootNode.findAll({
    rule: {
      // Use AST-based approach to find commas inside except clauses
      pattern: ",",
      inside: {
        kind: "except_clause",
      },
    },
  });

  const edits = nodes.map((node) => {
    // Replace the comma with "as"
    return node.replace(" as");
  });

  const newSource = rootNode.commitEdits(edits);
  return newSource;
};

export default transform;

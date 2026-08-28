import type { Codemod, Edit } from "codemod:ast-grep";
import type Toml from "codemod:ast-grep/langs/toml";

const codemod: Codemod<Toml> = async (root) => {
  const rootNode = root.root();

  const packageManagerPairs = rootNode.findAll({
    rule: {
      kind: "pair",
      has: {
        kind: "bare_key",
        regex: "^package-manager$",
      },
    },
  });

  const edits = packageManagerPairs
    .map((node) => {
      if (node.text() === 'package-manager = "pnpm@9.0.0"') {
        return null;
      }
      return node.replace('package-manager = "pnpm@9.0.0"');
    })
    .filter(Boolean);

  if (edits.length === 0) {
    return null;
  }

  return rootNode.commitEdits(edits as Edit[]);
};

export default codemod;

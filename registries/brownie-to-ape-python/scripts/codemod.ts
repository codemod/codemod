import type { Transform } from "codemod:ast-grep";
import type PY from "codemod:ast-grep/langs/python";

const transform: Transform<PY> = async (root) => {
  const rootNode = root.root();
  const edits = [];

  // 1. Migrate Imports
  const imports = rootNode.findAll({
    rule: {
      pattern: "from brownie import $$$PKGS",
    },
  });
  for (const node of imports) {
    edits.push(node.replace("from ape import accounts, project"));
  }

  // 2. Migrate Deployment Pattern
  const deployNodes = rootNode.findAll({
    rule: {
      pattern: "$CONTRACT.deploy($$$ARGS, {'from': $ACCT})",
    },
  });
  for (const node of deployNodes) {
    const contract = node.getMatch("CONTRACT")?.text();
    const args = node.getMatch("ARGS")?.text();
    const acct = node.getMatch("ACCT")?.text();
    if (contract && args && acct) {
      edits.push(node.replace(`${acct}.deploy(project.${contract}, ${args})`));
    }
  }

  return rootNode.commitEdits(edits);
};

export default transform;

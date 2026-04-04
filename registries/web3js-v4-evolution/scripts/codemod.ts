import type { Transform } from "codemod:ast-grep";
import type JS from "codemod:ast-grep/langs/javascript";

const transform: Transform<JS> = async (root) => {
  const rootNode = root.root();
  const edits = [];

  // 1. Migrate CommonJS Imports
  // From: const Web3 = require('web3');
  // To: const { Web3 } = require('web3');
  const cjsImports = rootNode.findAll({
    rule: {
      pattern: "const $NAME = require('web3')",
    },
  });
  for (const node of cjsImports) {
    const name = node.getMatch("NAME")?.text();
    if (name && name !== "{ Web3 }") {
      edits.push(node.replace(`const { ${name} } = require('web3')`));
    }
  }

  // 2. Migrate ES6 Imports
  // From: import Web3 from 'web3';
  // To: import { Web3 } from 'web3';
  const es6Imports = rootNode.findAll({
    rule: {
      pattern: "import $NAME from 'web3'",
    },
  });
  for (const node of es6Imports) {
    const name = node.getMatch("NAME")?.text();
    if (name && !name.startsWith("{")) {
      edits.push(node.replace(`import { ${name} } from 'web3'`));
    }
  }

  // 3. Enforce 'new' keyword for Contract instantiation
  // Web3.js v4 requires `new web3.eth.Contract`
  const contractNodes = rootNode.findAll({
    rule: {
      pattern: "$W.eth.Contract($$$ARGS)",
    },
  });
  for (const node of contractNodes) {
    // Check if it's already preceded by 'new'. 
    // If the parent is a NewExpression, it's fine.
    const parent = node.parent();
    if (parent && parent.kind() !== "new_expression") {
      const w = node.getMatch("W")?.text() || "web3";
      const args = node.getMatch("ARGS")?.text() || "";
      edits.push(node.replace(`new ${w}.eth.Contract(${args})`));
    }
  }

  // 4. Update Event Listeners (provider.on('close' -> 'disconnect'))
  const eventNodes = rootNode.findAll({
    rule: {
      pattern: "$P.on('close', $$$ARGS)",
    },
  });
  for (const node of eventNodes) {
    const p = node.getMatch("P")?.text();
    const args = node.getMatch("ARGS")?.text() || "";
    edits.push(node.replace(`${p}.on('disconnect', ${args})`));
  }

  return rootNode.commitEdits(edits);
};

export default transform;

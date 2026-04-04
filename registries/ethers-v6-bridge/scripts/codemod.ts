import type { Transform } from "codemod:ast-grep";
import type JS from "codemod:ast-grep/langs/javascript";

const transform: Transform<JS> = async (root) => {
  const rootNode = root.root();
  const edits = [];
  
  const stats = {
    deterministic_migrations: 0,
    ambiguous_skipped: 0,
  };

  // Helper: Traces types from local definitions OR JSDoc annotations
  const isBigNumberOrBigInt = (node: any): boolean => {
    const text = node.text();
    if (text.includes("BigNumber.from") || text.includes("BigInt(") || /^\d+n$/.test(text)) {
      return true;
    }

    // 1. Search for Variable Definition + JSDoc Inference
    const definition = rootNode.find({
      rule: {
        pattern: `const ${text} = $$$INIT`,
      }
    });

    if (definition) {
      const defText = definition.text();
      // Check for constructor usage
      if (defText.includes("BigNumber") || defText.includes("BigInt")) return true;
      
      // 2. Check for JSDoc comment preceding the definition
      const prevSibling = definition.prev();
      if (prevSibling && prevSibling.kind() === "comment" && prevSibling.text().includes("@type {BigNumber}")) {
        return true;
      }
    }

    return false;
  };

  // Operator Map for Chained Calls
  const opMap: Record<string, string> = {
    add: "+",
    sub: "-",
    mul: "*",
    div: "/",
    eq: "==",
    gt: ">",
    lt: "<"
  };

  // RECURSIVE PATTERN: Handles A.add(B).mul(C)
  const matches = rootNode.findAll({
    rule: {
      any: [
        { pattern: `$A.add($B)` },
        { pattern: `$A.sub($B)` },
        { pattern: `$A.mul($B)` },
        { pattern: `$A.div($B)` },
        { pattern: `$A.eq($B)` },
        { pattern: `$A.gt($B)` },
        { pattern: `$A.lt($B)` },
      ]
    },
  });

  for (const node of matches) {
    // Only process if this is the OUTERMOST math call in a chain to avoid double-replacing
    if (node.parent()?.text().includes(".add(") || node.parent()?.text().includes(".mul(")) {
       continue; 
    }

    const a = node.getMatch("A");
    const b = node.getMatch("B");
    const method = node.text().split('(')[0].split('.').pop() || "";
    const op = opMap[method];

    if (a && b && op) {
      if (isBigNumberOrBigInt(a)) {
        edits.push(node.replace(`${a.text()} ${op} BigInt(${b.text()})`));
        stats.deterministic_migrations++;
      } else {
        let statement = node;
        while (statement.parent() && statement.parent().kind() !== "expression_statement") {
          statement = statement.parent();
        }
        const warning = `// [SENTINEL-LOW-CONFIDENCE]: Verify BigInt conversion for ${a.text()}\n`;
        edits.push(statement.replace(warning + statement.text()));
        stats.ambiguous_skipped++;
      }
    }
  }

  const total = stats.deterministic_migrations + stats.ambiguous_skipped;
  if (total > 0) {
    console.info(JSON.stringify({ ...stats, confidence_score: ((stats.deterministic_migrations / total) * 100).toFixed(2) + "%" }, null, 2));
  }

  return rootNode.commitEdits(edits);
};

export default transform;

import type { Transform } from "codemod:ast-grep";
import type Rust from "codemod:ast-grep/langs/rust";

const transform: Transform<Rust> = async (root) => {
  const rootNode = root.root();

  // Find all println! macro calls
  const nodes = rootNode.findAll({
    rule: {
      pattern: "println!($$$ARGS)",
    },
  });

  if (nodes.length === 0) {
    return rootNode.text(); // No changes needed
  }

  // Check if log::info is already imported
  const hasLogImport = rootNode.find({
    rule: {
      any: [
        { pattern: "use log::info;" },
        { pattern: "use log::*;" },
        { pattern: "use log::{$$$, info, $$$};" },
      ],
    },
  });

  const edits = [];

  // Add import if not present
  if (!hasLogImport) {
    // Find the best place to insert the import
    const firstUseStatement = rootNode.find({
      rule: { pattern: "use $$$;" },
    });

    if (firstUseStatement) {
      // Insert before the first use statement
      edits.push(
        firstUseStatement.replace(`use log::info;
${firstUseStatement.text()}`),
      );
    } else {
      // No existing imports, add at the beginning of the file
      const firstItem = rootNode.children()[0];
      if (firstItem) {
        edits.push(
          firstItem.replace(`use log::info;

${firstItem.text()}`),
        );
      }
    }
  }

  // Transform all println! calls
  nodes.forEach((node) => {
    const args =
      node
        .getMultipleMatches("ARGS")
        .map((x) => x.text())
        .filter((x) => x !== ",")
        .join(", ") || "";
    edits.push(node.replace(`log::info!(${args})`));
  });

  const newSource = rootNode.commitEdits(edits);
  return newSource;
};

export default transform;

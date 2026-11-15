import { parse } from "codemod/ast-grep";
import type Rust from "codemod/ast-grep/langs/rust";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.rs"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace println! with log::info!", forEachFile(files, (file) => {
    const sgRoot = parse<Rust>(file, { language: "rust" });
    const rootNode = sgRoot.root();

    // Find all println! macro calls
    const nodes = rootNode.findAll({
      rule: {
        pattern: "println!($$$ARGS)",
      },
    });
    if (nodes.length === 0) return;

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
    for (const node of nodes) {
      const args = node
        .getMultipleMatches("ARGS")
        .map((x) => x.text())
        .filter((x) => x !== ",")
        .join(", ");
      edits.push(node.replace(`log::info!(${args})`));
    }

    file.write(rootNode.commitEdits(edits));
  }));
}

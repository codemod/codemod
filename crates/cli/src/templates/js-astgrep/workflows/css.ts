import { parse } from "codemod/ast-grep";
import type CSS from "codemod/ast-grep/langs/css";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.css"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Remove vendor-prefixed properties", forEachFile(files, (file) => {
    const sgRoot = parse<CSS>(file, { language: "css" });
    const rootNode = sgRoot.root();

    // Vendor-prefixed properties that have standard equivalents
    const vendorPrefixedDeclarations = rootNode.findAll({
      rule: {
        kind: "declaration",
        has: {
          kind: "property_name",
          regex: "^(-webkit-|-moz-|-ms-|-o-)(border-radius|box-shadow|transition|transform|background)",
        },
      },
    });

    const edits = vendorPrefixedDeclarations.map((declaration) => declaration.replace(""));

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

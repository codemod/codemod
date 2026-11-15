import { parse, type Edit } from "codemod/ast-grep";
import type Json from "codemod/ast-grep/langs/json";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.json"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Rename snake_case keys to camelCase", forEachFile(files, (file) => {
    const sgRoot = parse<Json>(file, { language: "json" });
    const rootNode = sgRoot.root();

    // All property names
    const keyNodes = rootNode.findAll({
      rule: {
        kind: "string",
        inside: {
          kind: "pair",
          field: "key",
        },
      },
    });

    const edits = keyNodes
      .map((node) => {
        // The key without its quotes
        const keyContent = node.text().slice(1, -1);
        if (!keyContent.includes("_") || /^[a-z][a-zA-Z0-9]*$/.test(keyContent)) {
          return null;
        }
        const camelCaseKey = keyContent
          .split("_")
          .map((word, index) =>
            index === 0
              ? word.toLowerCase()
              : word.charAt(0).toUpperCase() + word.slice(1).toLowerCase(),
          )
          .join("");
        return node.replace(`"${camelCaseKey}"`);
      })
      .filter(Boolean) as Edit[];

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

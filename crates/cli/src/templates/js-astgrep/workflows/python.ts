import { parse } from "codemod/ast-grep";
import type Python from "codemod/ast-grep/langs/python";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.py"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace Python 2 except clauses", forEachFile(files, (file) => {
    const sgRoot = parse<Python>(file, { language: "python" });
    const rootNode = sgRoot.root();

    // Find all Python 2-style exception handlers (using comma instead of 'as')
    const nodes = rootNode.findAll({
      rule: {
        pattern: ",",
        inside: {
          kind: "except_clause",
        },
      },
    });

    // Replace the comma with "as"
    const edits = nodes.map((node) => node.replace(" as"));

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

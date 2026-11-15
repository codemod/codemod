import { parse } from "codemod/ast-grep";
import type Ruby from "codemod/ast-grep/langs/ruby";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.rb"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace puts with logger", forEachFile(files, (file) => {
    const sgRoot = parse<Ruby>(file, { language: "ruby" });
    const rootNode = sgRoot.root();

    const nodes = rootNode.findAll({
      rule: {
        pattern: "puts $ARGS",
      },
    });

    const edits = nodes.map((node) => {
      const args = node.getMatch("ARGS")?.text();
      return node.replace(`logger.info(${args})`);
    });

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

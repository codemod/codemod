import { parse } from "codemod/ast-grep";
import type C from "codemod/ast-grep/langs/c";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.{c,h}"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace console logging with logger", forEachFile(files, (file) => {
    const sgRoot = parse<C>(file, { language: "c" });
    const rootNode = sgRoot.root();

    const nodes = rootNode.findAll({
      rule: {
        any: [{ pattern: "console.log($ARG)" }, { pattern: "console.debug($ARG)" }],
      },
    });

    const edits = nodes.map((node) => {
      const arg = node.getMatch("ARG")?.text();
      return node.replace(`logger.log(${arg})`);
    });

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

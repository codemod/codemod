import { parse } from "codemod/ast-grep";
import type C from "codemod/ast-grep/langs/c";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.{c,h}"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Bound sprintf calls", forEachFile(files, (file) => {
    const sgRoot = parse<C>(file, { language: "c" });
    const rootNode = sgRoot.root();

    // sprintf(buffer, format, ...) can write past the end of buffer.
    const calls = rootNode.findAll({
      rule: { pattern: "sprintf($BUF, $$$ARGS)" },
    });

    // snprintf(buffer, sizeof(buffer), format, ...) cannot.
    const edits = calls.map((node) => {
      const buffer = node.getMatch("BUF")?.text();
      const args = node
        .getMultipleMatches("ARGS")
        .filter((arg) => arg.isNamed())
        .map((arg) => arg.text())
        .join(", ");
      return node.replace(`snprintf(${buffer}, sizeof(${buffer}), ${args})`);
    });

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

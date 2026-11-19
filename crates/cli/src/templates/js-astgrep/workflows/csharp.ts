import { parse } from "codemod/ast-grep";
import type CSharp from "codemod/ast-grep/langs/c_sharp";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.cs"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace Console writes with Logger", forEachFile(files, (file) => {
    const sgRoot = parse<CSharp>(file, { language: "csharp" });
    const rootNode = sgRoot.root();

    const nodes = rootNode.findAll({
      rule: {
        any: [{ pattern: "Console.WriteLine($ARG)" }, { pattern: "Console.Write($ARG)" }],
      },
    });

    const edits = nodes.map((node) => {
      const arg = node.getMatch("ARG")?.text();
      return node.replace(`Logger.Log(${arg})`);
    });

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

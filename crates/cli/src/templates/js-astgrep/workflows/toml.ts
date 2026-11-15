import { parse } from "codemod/ast-grep";
import type Toml from "codemod/ast-grep/langs/toml";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.toml"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Pin the package manager", forEachFile(files, (file) => {
    const sgRoot = parse<Toml>(file, { language: "toml" });
    const rootNode = sgRoot.root();

    const packageManagerPairs = rootNode.findAll({
      rule: {
        kind: "pair",
        has: {
          kind: "bare_key",
          regex: "^package-manager$",
        },
      },
    });

    const edits = packageManagerPairs
      .filter((node) => node.text() !== 'package-manager = "pnpm@9.0.0"')
      .map((node) => node.replace('package-manager = "pnpm@9.0.0"'));

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

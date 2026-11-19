import { parse } from "codemod/ast-grep";
import type JS from "codemod/ast-grep/langs/javascript";
import { forEachFile, step } from "codemod/workflow";

// `parse` reads each file as the language you give it:
// - "tsx" for files with JSX (.tsx, .jsx);
// - "typescript" for TypeScript without JSX;
// - "javascript" for plain JavaScript.
// When you change it, change the type given to `parse` and the files below to match.

// The files the transform runs on.
const files = {
  include: ["**/*.{js,jsx,mjs,cjs}"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace var with const", forEachFile(files, (file) => {
    const sgRoot = parse<JS>(file, { language: "javascript" });
    const rootNode = sgRoot.root();

    const nodes = rootNode.findAll({
      rule: {
        pattern: "var $VAR = $VALUE",
      },
    });

    const edits = nodes.map((node) => {
      const varName = node.getMatch("VAR")?.text();
      const value = node.getMatch("VALUE")?.text();
      // The matched declaration includes its trailing semicolon (when present), so keep it.
      const semicolon = node.text().endsWith(";") ? ";" : "";
      return node.replace(`const ${varName} = ${value}${semicolon}`);
    });

    // Without edits the file is left as it is.
    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

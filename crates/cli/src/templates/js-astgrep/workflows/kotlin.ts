import { parse } from "codemod/ast-grep";
import type Kotlin from "codemod/ast-grep/langs/kotlin";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.kt"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace null checks with the elvis operator", forEachFile(files, (file) => {
    const sgRoot = parse<Kotlin>(file, { language: "kotlin" });
    const rootNode = sgRoot.root();

    // Null checks followed by a return
    const simpleNullChecks = rootNode.findAll({
      rule: {
        pattern: "if ($VAR == null) { return $DEFAULT }",
      },
    });

    const edits = simpleNullChecks.map((node) => {
      const variableName = node.getMatch("VAR")?.text();
      const defaultValue = node.getMatch("DEFAULT")?.text();
      return node.replace(`val ${variableName}Safe = ${variableName} ?: return ${defaultValue}`);
    });

    // Negated null checks with nested operations
    const negatedNullChecks = rootNode.findAll({
      rule: {
        pattern: `
          if ($VAR != null) {
            $BODY
          } else {
            return $DEFAULT
          }
        `,
      },
    });

    for (const node of negatedNullChecks) {
      const variableName = node.getMatch("VAR")?.text();
      const body = node.getMatch("BODY")?.text();
      const defaultValue = node.getMatch("DEFAULT")?.text();
      edits.push(
        node.replace(`val ${variableName}Safe = ${variableName} ?: return ${defaultValue}
            ${body}`),
      );
    }

    // Inline null checks
    const inlineNullChecks = rootNode.findAll({
      rule: {
        pattern: "if ($VAR == null) return $DEFAULT",
      },
    });

    for (const node of inlineNullChecks) {
      const variableName = node.getMatch("VAR")?.text();
      const defaultValue = node.getMatch("DEFAULT")?.text();
      edits.push(
        node.replace(`val ${variableName}Safe = ${variableName} ?: return ${defaultValue}`),
      );
    }

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

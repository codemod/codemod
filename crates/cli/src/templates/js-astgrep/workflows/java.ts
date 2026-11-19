import { parse } from "codemod/ast-grep";
import type Java from "codemod/ast-grep/langs/java";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.java"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace collection loops with streams", forEachFile(files, (file) => {
    const sgRoot = parse<Java>(file, { language: "java" });
    const rootNode = sgRoot.root();

    // For-each loops that filter a collection into another collection
    const filterMapLoops = rootNode.findAll({
      rule: {
        pattern: `
          for ($TYPE $VAR : $COLLECTION) {
            if ($CONDITION) {
              $TARGET.add($EXPRESSION);
            }
          }
        `,
      },
    });

    const edits = filterMapLoops.map((node) => {
      const variable = node.getMatch("VAR")?.text();
      const collection = node.getMatch("COLLECTION")?.text();
      const condition = node.getMatch("CONDITION")?.text();
      const target = node.getMatch("TARGET")?.text();
      const expression = node.getMatch("EXPRESSION")?.text();

      return node.replace(`
      ${collection}.stream()
          .filter(${variable} -> ${condition})
          .map(${variable} -> ${expression})
          .forEach(item -> ${target}.add(item));
    `);
    });

    // For-each loops that map one collection to another
    const simpleMappingLoops = rootNode.findAll({
      rule: {
        pattern: `
          for ($TYPE $VAR : $COLLECTION) {
            $TARGET.add($EXPRESSION);
          }
        `,
      },
    });

    const moreEdits = simpleMappingLoops.map((node) => {
      const variable = node.getMatch("VAR")?.text();
      const collection = node.getMatch("COLLECTION")?.text();
      const target = node.getMatch("TARGET")?.text();
      const expression = node.getMatch("EXPRESSION")?.text();

      return node.replace(`
      ${collection}.stream()
          .map(${variable} -> ${expression})
          .forEach(item -> ${target}.add(item));
    `);
    });

    if (edits.length + moreEdits.length === 0) return;
    file.write(rootNode.commitEdits([...edits, ...moreEdits]));
  }));
}

import { parse } from "codemod/ast-grep";
import type Angular from "codemod/ast-grep/langs/angular";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.html"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace ngClass ternaries with class bindings", forEachFile(files, (file) => {
    const sgRoot = parse<Angular>(file, { language: "angular" });
    const rootNode = sgRoot.root();

    // Property bindings, such as [ngClass]="[...]"
    const bindings = rootNode.findAll({
      rule: {
        kind: "attribute",
        has: {
          kind: "property_binding",
          has: {
            kind: "expression",
            pattern: "$EXPRESSIONS",
          },
        },
      },
    });

    const edits = bindings.map((node) => {
      // The expressions inside the array, one per ternary
      const ternaryExpressions = node
        .getMatch("EXPRESSIONS")
        ?.text()
        .split(",")
        .map((expr) => expr.trim())
        .filter((expr) => expr.length > 0);

      // Each "condition ? 'trueValue' : ''" becomes a template string part
      const templateParts = ternaryExpressions
        ?.map((ternary) => {
          const ternaryMatch = ternary.match(/([^?]+) *\? *([^:]+) *: *(.*)/);
          if (!ternaryMatch) return null;

          let conditionPart = ternaryMatch[1]?.trim();
          const truePart = ternaryMatch[2]?.trim();
          const falsePart = ternaryMatch[3]?.trim();
          if (conditionPart?.startsWith("[")) {
            conditionPart = conditionPart.slice(1);
          }
          return "${" + conditionPart + " ? " + truePart + " : " + falsePart + "}";
        })
        .filter(Boolean);

      return node.replace(`[class]="\`${templateParts?.join(" ")}\`.trim()"`);
    });

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

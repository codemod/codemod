import { parse } from "codemod/ast-grep";
import type Go from "codemod/ast-grep/langs/go";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.go"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace fmt printing with log", forEachFile(files, (file) => {
    const sgRoot = parse<Go>(file, { language: "go" });
    const rootNode = sgRoot.root();

    // Calls to fmt.<name>
    const fmtCalls = (name: string) =>
      rootNode.findAll({
        rule: {
          kind: "call_expression",
          has: {
            field: "function",
            all: [
              { has: { field: "operand", regex: "^fmt$" } },
              { has: { field: "field", regex: `^${name}$` } },
            ],
          },
        },
      });

    const edits = [
      // Replace fmt.Println with log.Info
      ...fmtCalls("Println").map((node) => {
        const argsNode = node.field("arguments");
        if (argsNode && argsNode.children().some((child) => child.isNamed())) {
          const args = argsNode
            .children()
            .filter((child) => child.isNamed())
            .map((child) => child.text())
            .join(", ");
          return node.replace(`log.Info("message", "args", []interface{}{${args}})`);
        }
        return node.replace('log.Info("message")');
      }),

      // Replace fmt.Printf with log.Infof
      ...fmtCalls("Printf").map((node) => {
        const argsNode = node.field("arguments");
        if (argsNode && argsNode.children().some((child) => child.isNamed())) {
          const args = argsNode
            .children()
            .filter((child) => child.isNamed())
            .map((child) => child.text())
            .join(", ");
          return node.replace(`log.Infof(${args})`);
        }
        return node.replace('log.Info("message")');
      }),
    ];

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

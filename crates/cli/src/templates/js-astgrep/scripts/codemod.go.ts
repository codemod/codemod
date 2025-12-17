import type { Transform } from "codemod:ast-grep";
import type Go from "codemod:ast-grep/langs/go";

const transform: Transform<Go> = async (root) => {
  const rootNode = root.root();

  // Find all fmt.Println calls
  const printlnNodes = rootNode.findAll({
    rule: {
      kind: "call_expression",
      has: {
        field: "function",
        all: [
          {
            has: {
              field: "operand",
              regex: "^fmt$",
            },
          },
          {
            has: {
              field: "field",
              regex: "^Println$",
            },
          },
        ],
      },
    },
  });

  // Find all fmt.Printf calls
  const printfNodes = rootNode.findAll({
    rule: {
      kind: "call_expression",
      has: {
        field: "function",
        all: [
          {
            has: {
              field: "operand",
              regex: "^fmt$",
            },
          },
          {
            has: {
              field: "field",
              regex: "^Printf$",
            },
          },
        ],
      },
    },
  });

  const edits = [
    // Replace fmt.Println with log.Info
    ...printlnNodes.map((node) => {
      const argsNode = node.field("arguments");
      if (argsNode && argsNode.children().length > 0) {
        const args = argsNode
          .children()
          .map((child) => child.text())
          .join(", ");
        return node.replace(`log.Info("message", "args", []interface{}{${args}})`);
      }
      return node.replace('log.Info("message")');
    }),

    // Replace fmt.Printf with log.Infof
    ...printfNodes.map((node) => {
      const argsNode = node.field("arguments");
      if (argsNode && argsNode.children().length > 0) {
        const args = argsNode
          .children()
          .map((child) => child.text())
          .join(", ");
        return node.replace(`log.Infof(${args})`);
      }
      return node.replace('log.Info("message")');
    }),
  ];

  const newSource = rootNode.commitEdits(edits);
  return newSource;
};

export default transform;

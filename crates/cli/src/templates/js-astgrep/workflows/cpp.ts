import { parse } from "codemod/ast-grep";
import type Cpp from "codemod/ast-grep/langs/cpp";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.{cpp,cc,cxx,hpp,hh,hxx}"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace raw pointers with smart pointers", forEachFile(files, (file) => {
    const sgRoot = parse<Cpp>(file, { language: "cpp" });
    const rootNode = sgRoot.root();

    // Raw pointer declarations with new
    const ptrDeclarations = rootNode.findAll({
      rule: {
        pattern: "$TYPE* $VAR = new $CONSTRUCTOR($$$ARGS)",
      },
    });

    // Array allocations
    const arrayDeclarations = rootNode.findAll({
      rule: {
        pattern: "$TYPE* $VAR = new $TYPE[$SIZE]",
      },
    });

    // Delete statements
    const deleteStatements = rootNode.findAll({
      rule: {
        any: [{ pattern: "delete $VAR;" }, { pattern: "delete[] $VAR;" }],
      },
    });

    const edits = [
      // Convert new allocations to make_unique
      ...ptrDeclarations.map((node) => {
        const type = node.getMatch("TYPE")?.text();
        const varName = node.getMatch("VAR")?.text();
        const argsText = node
          .getMultipleMatches("ARGS")
          .filter((arg) => arg.isNamed())
          .map((arg) => arg.text())
          .join(", ");
        return node.replace(`auto ${varName} = std::make_unique<${type}>(${argsText});`);
      }),

      // Convert array allocations to vectors
      ...arrayDeclarations.map((node) => {
        const type = node.getMatch("TYPE")?.text();
        const varName = node.getMatch("VAR")?.text();
        const size = node.getMatch("SIZE")?.text();
        return node.replace(`std::vector<${type}> ${varName}(${size});`);
      }),

      // Remove delete statements
      ...deleteStatements.map((node) => node.replace("// Automatic cleanup with smart pointers")),
    ];

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

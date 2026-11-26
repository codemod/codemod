import { parse } from "codemod/ast-grep";
import type PHP from "codemod/ast-grep/langs/php";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.php"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace mysql_* calls with PDO", forEachFile(files, (file) => {
    const sgRoot = parse<PHP>(file, { language: "php" });
    const rootNode = sgRoot.root();

    // Calls to the function `name`
    const calls = (name: string) =>
      rootNode.findAll({
        rule: {
          kind: "function_call_expression",
          has: { field: "function", kind: "name", regex: `^${name}$` },
        },
      });

    const edits = [
      // Convert mysql_query to a PDO query
      ...calls("mysql_query").map((node) => {
        const argsText = node.field("arguments")?.text() ?? "()";
        return node.replace(`$pdo->query${argsText}`);
      }),

      // Convert mysql_fetch_array to a PDO fetch
      ...calls("mysql_fetch_array").map((node) =>
        node.replace(`$result->fetch(PDO::FETCH_BOTH)`),
      ),

      // Convert mysql_fetch_assoc to a PDO fetch
      ...calls("mysql_fetch_assoc").map((node) =>
        node.replace(`$result->fetch(PDO::FETCH_ASSOC)`),
      ),
    ];

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

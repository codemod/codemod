import { parse } from "codemod/ast-grep";
import type Elixir from "codemod/ast-grep/langs/elixir";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.{ex,exs}"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace IO calls with Logger", forEachFile(files, (file) => {
    const sgRoot = parse<Elixir>(file, { language: "elixir" });
    const rootNode = sgRoot.root();

    // IO.puts calls
    const ioPutsNodes = rootNode.findAll({
      rule: {
        pattern: "IO.puts($ARG)",
      },
    });

    // IO.inspect calls
    const ioInspectNodes = rootNode.findAll({
      rule: {
        pattern: "IO.inspect($$$ARGS)",
      },
    });

    const edits = [
      ...ioPutsNodes.map((node) => {
        const arg = node.getMatch("ARG")?.text();
        return node.replace(`Logger.info(${arg})`);
      }),
      ...ioInspectNodes.map((node) => {
        const args = node
          .getMultipleMatches("ARGS")
          .map((x) => x.text())
          .filter((x) => x !== ",")
          .join(", ");
        // Logger.debug keeps the inspection behaviour
        return node.replace(`Logger.debug(${args})`);
      }),
    ];

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

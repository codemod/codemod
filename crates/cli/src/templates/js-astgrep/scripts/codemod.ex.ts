import type { SgRoot } from "codemod:ast-grep";
import type Elixir from "codemod:ast-grep/langs/elixir";

async function transform(root: SgRoot<Elixir>): Promise<string> {
  const rootNode = root.root();

  // Find IO.puts calls
  const ioPutsNodes = rootNode.findAll({
    rule: {
      pattern: "IO.puts($ARG)"
    },
  });

  // Find IO.inspect calls  
  const ioInspectNodes = rootNode.findAll({
    rule: {
      pattern: "IO.inspect($$$ARGS)"
    },
  });

  const edits = [
    ...ioPutsNodes.map(node => {
      const arg = node.getMatch("ARG")?.text();
      return node.replace(`Logger.info(${arg})`);
    }),
    ...ioInspectNodes.map(node => {
      const args = node.getMultipleMatches("ARGS")
        .map(x => x.text())
        .filter(x => x !== ',')
        .join(', ');
      // For IO.inspect, we'll convert to Logger.debug to preserve the inspection behavior
      return node.replace(`Logger.debug(${args})`);
    })
  ];

  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default transform;

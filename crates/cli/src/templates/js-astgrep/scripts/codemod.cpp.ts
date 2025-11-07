import type { SgRoot } from "codemod:ast-grep";
import type CPP from "codemod:ast-grep/langs/cpp";

async function transform(root: SgRoot<CPP>): Promise<string> {
  const rootNode = root.root();

  // Find raw pointer declarations with new
  const ptrDeclarations = rootNode.findAll({
    rule: {
      pattern: "$TYPE* $VAR = new $CONSTRUCTOR($$$ARGS)"
    },
  });

  // Find array allocations
  const arrayDeclarations = rootNode.findAll({
    rule: {
      pattern: "$TYPE* $VAR = new $TYPE[$SIZE]"
    },
  });

  // Find delete statements to remove
  const deleteStatements = rootNode.findAll({
    rule: {
      any: [
        { pattern: "delete $VAR" },
        { pattern: "delete[] $VAR" }
      ]
    },
  });

  const edits = [
    // Convert new allocations to make_unique
    ...ptrDeclarations.map(node => {
      const type = node.getMatch("TYPE")?.text();
      const varName = node.getMatch("VAR")?.text();
      const constructor = node.getMatch("CONSTRUCTOR")?.text();
      const args = node.getMatch("ARGS");
      const argsText = args ? args.text() : "";
      
      return node.replace(`auto ${varName} = std::make_unique<${type}>(${argsText})`);
    }),
    
    // Convert array allocations to vector or array
    ...arrayDeclarations.map(node => {
      const type = node.getMatch("TYPE")?.text();
      const varName = node.getMatch("VAR")?.text();
      const size = node.getMatch("SIZE")?.text();
      
      return node.replace(`std::vector<${type}> ${varName}(${size})`);
    }),
    
    // Remove delete statements
    ...deleteStatements.map(node => {
      return node.replace("// Automatic cleanup with smart pointers");
    })
  ];

  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default transform;

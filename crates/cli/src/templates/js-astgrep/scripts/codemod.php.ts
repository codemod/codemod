import type { SgRoot } from "codemod:ast-grep";
import type PHP from "codemod:ast-grep/langs/php";

async function transform(root: SgRoot<PHP>): Promise<string> {
  const rootNode = root.root();

  // Find all mysql_query function calls
  const queryNodes = rootNode.findAll({
    rule: {
      kind: "function_call_expression",
      all: [
        {
          has: {
            field: "function",
            kind: "name",
            regex: "^mysql_query$"
          }
        }
      ]
    },
  });

  // Find all mysql_fetch_array function calls
  const fetchArrayNodes = rootNode.findAll({
    rule: {
      kind: "function_call_expression",
      all: [
        {
          has: {
            field: "function", 
            kind: "name",
            regex: "^mysql_fetch_array$"
          }
        }
      ]
    },
  });

  // Find all mysql_fetch_assoc function calls
  const fetchAssocNodes = rootNode.findAll({
    rule: {
      kind: "function_call_expression",
      all: [
        {
          has: {
            field: "function",
            kind: "name", 
            regex: "^mysql_fetch_assoc$"
          }
        }
      ]
    },
  });

  const edits = [
    // Convert mysql_query to PDO query
    ...queryNodes.map(node => {
      // Get the arguments field directly from the function call
      const argsNode = node.field("arguments");
      const argsText = argsNode ? argsNode.text() : "()";
      return node.replace(`$pdo->query${argsText}`);
    }),
    
    // Convert mysql_fetch_array to PDO fetch
    ...fetchArrayNodes.map(node => {
      // Get the first argument (the result variable) from the arguments
      const argsNode = node.field("arguments");
      return node.replace(`$result->fetch(PDO::FETCH_BOTH)`);
    }),
    
    // Convert mysql_fetch_assoc to PDO fetch
    ...fetchAssocNodes.map(node => {
      return node.replace(`$result->fetch(PDO::FETCH_ASSOC)`);
    })
  ];

  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default transform;

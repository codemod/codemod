import type { SgRoot } from "codemod:ast-grep";
import type Angular from "codemod:ast-grep/langs/angular";

async function transform(root: SgRoot<Angular>): Promise<string> {
  const rootNode = root.root();

  // Find all ngClass array bindings
  const ngClassArrayNodes = rootNode.findAll({
    rule: {
      kind: "attribute",
      has: {
        kind: "property_binding",
        has: {
          kind: "expression",
          pattern: "$EXPRESSIONS"
        }
      }
    }
  });

  const edits = ngClassArrayNodes.map(node => {
    // Get the expressions inside the array
    const arrayExpressionsText = node.getMatch("EXPRESSIONS")?.text();
    
    // Handle cases where this is not an array of ternaries
    // if (!arrayExpressionsText.includes(' ? ')) {
    //   return null;
    // }
    
    // Parse the expressions (this is the complex part)
    // We need to process each ternary expression in the array
    const ternaryExpressions = arrayExpressionsText
      ?.split(',')
      ?.map(expr => expr.trim())
      ?.filter(expr => expr.length > 0);
    
    // Transform each ternary expression into a template string part
    const templateParts = ternaryExpressions?.map(ternary => {
      // Simple regex to extract parts of the ternary
      // This assumes a basic format of "condition ? 'trueValue' : ''"
      const ternaryMatch = ternary.match(/([^?]+) *\? *([^:]+) *: *(.*)/);
      if (!ternaryMatch) return null;
      
      const conditionPart = ternaryMatch[1]?.trim();
      const truePart = ternaryMatch[2]?.trim();
      const falsePart = ternaryMatch[3]?.trim();
      
      // Create a template string segment
      return `\${${conditionPart} ? ${truePart} : ${falsePart}}`;
    }).filter(Boolean);
    
    // if (templateParts.length === 0) return null;
    
    // Construct the new class binding with template string
    const newClassBinding = `[class]="\`${templateParts?.join(' ')}\`.trim()"`;
    
    return node.replace(newClassBinding);
  }) // Remove null edits

  console.log(edits);
  
  // Apply all changes
  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default transform;

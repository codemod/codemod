import type { Edit, SgRoot } from "codemod:ast-grep";

async function transform(root: SgRoot): Promise<string | null> {
  const rootNode = root.root();
  
  // Find all string keys (property names)
  const keyNodes = rootNode.findAll({
    rule: {
      kind: "string",
      inside: {
        kind: "pair",
        field: "key"
      }
    }
  });

  const edits = keyNodes.map(node => {
    const keyText = node.text();
    
    // Remove quotes and check if it's snake_case
    const keyContent = keyText.slice(1, -1); // Remove surrounding quotes
    
    // Check if the key contains underscores and is not already camelCase
    if (keyContent.includes('_') && !keyContent.match(/^[a-z][a-zA-Z0-9]*$/)) {
      // Convert snake_case to camelCase
      const camelCaseKey = keyContent
        .split('_')
        .map((word, index) => {
          if (index === 0) {
            return word.toLowerCase();
          }
          return word.charAt(0).toUpperCase() + word.slice(1).toLowerCase();
        })
        .join('');
      
      // Return the replacement with quotes maintained
      return node.replace(`"${camelCaseKey}"`);
    }
    
    return null;
  }).filter(Boolean); // Remove null values

  if (edits.length === 0) {
    return null; // No changes needed
  }

  const newSource = rootNode.commitEdits(edits as Edit[]);
  return newSource;
}

export default transform;

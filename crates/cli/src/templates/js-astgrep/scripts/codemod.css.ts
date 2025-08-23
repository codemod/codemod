import type { SgRoot } from "codemod:ast-grep";

async function transform(root: SgRoot): Promise<string> {
  const rootNode = root.root();

  // Find vendor-prefixed properties that have standard equivalents
  const vendorPrefixedDeclarations = rootNode.findAll({
    rule: {
      kind: "declaration",
      has: {
        kind: "property_name",
        regex: "^(-webkit-|-moz-|-ms-|-o-)(border-radius|box-shadow|transition|transform|background)"
      }
    }
  });

  const edits = [];

  for (const declaration of vendorPrefixedDeclarations) {
    const propertyNameNode = declaration.find({
      rule: {
        kind: "property_name"
      }
    });

    if (propertyNameNode) {
      const propertyText = propertyNameNode.text();
      
      // Extract standard property name by removing vendor prefix
      let standardProperty = propertyText;
      if (propertyText.startsWith('-webkit-')) {
        standardProperty = propertyText.substring(8);
      } else if (propertyText.startsWith('-moz-')) {
        standardProperty = propertyText.substring(5);
      } else if (propertyText.startsWith('-ms-')) {
        standardProperty = propertyText.substring(4);
      } else if (propertyText.startsWith('-o-')) {
        standardProperty = propertyText.substring(3);
      }
      
      // Check if standard property exists in the same rule block
      const block = declaration.parent(); // Get the containing block
      if (block) {
        edits.push(declaration.replace(""));
      }
    }
  }

  // Apply all edits
  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default transform;

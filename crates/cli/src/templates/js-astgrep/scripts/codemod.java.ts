import type { Transform } from "codemod:ast-grep";
import type Java from "codemod:ast-grep/langs/java";

const transform: Transform<Java> = async (root) => {
  const rootNode = root.root();

  // Find for-each loops that iterate over a collection and add items to another collection
  const filterMapLoops = rootNode.findAll({
    rule: {
      pattern: `
        for ($TYPE $VAR : $COLLECTION) {
          if ($CONDITION) {
            $TARGET.add($EXPRESSION);
          }
        }
      `,
    },
  });

  const edits = filterMapLoops.map((node) => {
    const variable = node.getMatch("VAR")?.text();
    const collection = node.getMatch("COLLECTION")?.text();
    const condition = node.getMatch("CONDITION")?.text();
    const target = node.getMatch("TARGET")?.text();
    const expression = node.getMatch("EXPRESSION")?.text();

    // Transform to streams API
    return node.replace(`
      ${collection}.stream()
          .filter(${variable} -> ${condition})
          .map(${variable} -> ${expression})
          .forEach(item -> ${target}.add(item));
    `);
  });

  // Find simple for-each loops that just map one collection to another
  const simpleMappingLoops = rootNode.findAll({
    rule: {
      pattern: `
        for ($TYPE $VAR : $COLLECTION) {
          $TARGET.add($EXPRESSION);
        }
      `,
    },
  });

  const moreEdits = simpleMappingLoops.map((node) => {
    const variable = node.getMatch("VAR")?.text();
    const collection = node.getMatch("COLLECTION")?.text();
    const target = node.getMatch("TARGET")?.text();
    const expression = node.getMatch("EXPRESSION")?.text();

    // Transform to streams API
    return node.replace(`
      ${collection}.stream()
          .map(${variable} -> ${expression})
          .forEach(item -> ${target}.add(item));
    `);
  });

  // Apply all changes
  const newSource = rootNode.commitEdits([...edits, ...moreEdits]);
  return newSource;
};

export default transform;

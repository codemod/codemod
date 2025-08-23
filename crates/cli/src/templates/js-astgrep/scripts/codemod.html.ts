import type { SgRoot } from "codemod:ast-grep";
import type Html from "codemod:ast-grep/langs/html";

async function transform(root: SgRoot<Html>): Promise<string> {
  const rootNode = root.root();

  // Replace <center> tags with styled div
  const centerNodes = rootNode.findAll({
    rule: {
      pattern: "<center>$CONTENT</center>"
    }
  });

  let edits = centerNodes.map(node => {
    const content = node.getMatch("CONTENT")?.text();
    return node.replace(`<div style="text-align: center">${content}</div>`);
  });

  // Replace <font> tags with styled span
  const fontNodes = rootNode.findAll({
    rule: {
      pattern: "<font $ATTRS>$CONTENT</font>"
    }
  });

  edits = [
    ...edits,
    ...fontNodes.map(node => {
      const content = node.getMatch("CONTENT")?.text();
      const attrs = node.getMatch("ATTRS")?.text();
      
      // Convert color and size attributes to inline CSS
      let style = "";
      if (attrs?.includes('color="')) {
        const colorMatch = attrs.match(/color="([^"]*)"/);
        if (colorMatch) {
          style += `color: ${colorMatch[1]}; `;
        }
      }
      if (attrs?.includes('size="')) {
        const sizeMatch = attrs.match(/size="([^"]*)"/);
        if (sizeMatch) {
          const fontSize = parseInt(sizeMatch[1] ?? "0") * 4 + 8;
          style += `font-size: ${fontSize}px; `;
        }
      }
      
      return node.replace(`<span style="${style.trim()}">${content}</span>`);
    })
  ];

  // Apply all the changes
  const newSource = rootNode.commitEdits(edits);
  return newSource;
}

export default transform;

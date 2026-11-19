import { parse } from "codemod/ast-grep";
import type Html from "codemod/ast-grep/langs/html";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.html"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Replace center and font tags", forEachFile(files, (file) => {
    const sgRoot = parse<Html>(file, { language: "html" });
    const rootNode = sgRoot.root();

    // Replace <center> tags with a styled div
    const centerNodes = rootNode.findAll({
      rule: {
        pattern: "<center>$CONTENT</center>",
      },
    });

    const edits = centerNodes.map((node) => {
      const content = node.getMatch("CONTENT")?.text();
      return node.replace(`<div style="text-align: center">${content}</div>`);
    });

    // Replace <font> tags with a styled span
    const fontNodes = rootNode.findAll({
      rule: {
        pattern: "<font $ATTRS>$CONTENT</font>",
      },
    });

    for (const node of fontNodes) {
      const content = node.getMatch("CONTENT")?.text();
      const attrs = node.getMatch("ATTRS")?.text();

      // Convert color and size attributes to inline CSS
      let style = "";
      const colorMatch = attrs?.match(/color="([^"]*)"/);
      if (colorMatch) {
        style += `color: ${colorMatch[1]}; `;
      }
      const sizeMatch = attrs?.match(/size="([^"]*)"/);
      if (sizeMatch) {
        const fontSize = parseInt(sizeMatch[1] ?? "0") * 4 + 8;
        style += `font-size: ${fontSize}px; `;
      }

      edits.push(node.replace(`<span style="${style.trim()}">${content}</span>`));
    }

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

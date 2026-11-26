import { parse, type Edit } from "codemod/ast-grep";
import type Yaml from "codemod/ast-grep/langs/yaml";
import { forEachFile, step } from "codemod/workflow";

// The files the transform runs on.
const files = {
  include: ["**/*.{yaml,yml}"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/build/**"],
};

export default async function main() {
  await step("Update the compose file", forEachFile(files, (file) => {
    const sgRoot = parse<Yaml>(file, { language: "yaml" });
    const rootNode = sgRoot.root();

    // The version field
    const versionNodes = rootNode.findAll({
      rule: {
        kind: "block_mapping_pair",
        has: {
          kind: "flow_node",
          has: {
            kind: "plain_scalar",
            regex: "^version$",
          },
        },
      },
    });
    if (versionNodes.length === 0) return;

    // Update the version to 3.8
    const edits = versionNodes
      .map((node) => node.field("value")?.replace("'3.8'"))
      .filter(Boolean) as Edit[];

    // Comment out the deprecated 'links' sections
    const linkNodes = rootNode.findAll({
      rule: {
        kind: "block_mapping_pair",
        has: {
          field: "key",
          kind: "plain_scalar",
          regex: "^links$",
        },
      },
    });

    for (const node of linkNodes) {
      const commentedLinks = node
        .text()
        .split("\n")
        .map((line) => `# ${line} # deprecated: use networks instead`)
        .join("\n");
      edits.push(node.replace(commentedLinks));
    }

    // Services that had links get a networks section
    const servicesWithLinks = rootNode.findAll({
      rule: {
        kind: "block_mapping_pair",
        inside: {
          kind: "block_mapping_pair",
          has: {
            field: "key",
            kind: "plain_scalar",
            regex: "^services$",
          },
        },
        has: {
          kind: "block_mapping_pair",
          has: {
            field: "key",
            kind: "plain_scalar",
            regex: "^links$",
          },
        },
      },
    });

    for (const serviceNode of servicesWithLinks) {
      const serviceContent = serviceNode.text();
      if (!serviceContent.includes("networks:")) {
        edits.push(serviceNode.replace(serviceContent + "\n    networks:\n      - default"));
      }
    }

    if (edits.length === 0) return;
    file.write(rootNode.commitEdits(edits));
  }));
}

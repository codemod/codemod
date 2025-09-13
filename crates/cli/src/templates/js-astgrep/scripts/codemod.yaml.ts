import type { Edit, SgRoot } from "codemod:ast-grep";

async function transform(root: SgRoot): Promise<string | null> {
  const rootNode = root.root();

  // Find version field with value '2' or '2.x'
  const versionNodes = rootNode.findAll({
    rule: {
      kind: "block_mapping_pair",
      has: {
        kind: "flow_node",
        has: {
          kind: "plain_scalar",
          regex: "^version$"
        }
      }
    }
  });

  console.log(versionNodes);

  if (versionNodes.length === 0) {
    return null; // No version 2 found
  }

  let edits = [];

  // Update version to 3.8
  edits = versionNodes.map(node => {
    const valueNode = node.field("value");
    if (valueNode) {
      return valueNode.replace("'3.8'");
    }
    return null;
  }).filter(Boolean);

  // Find and remove deprecated 'links' sections
  const linkNodes = rootNode.findAll({
    rule: {
      kind: "block_mapping_pair",
      has: {
        field: "key",
        kind: "plain_scalar",
        regex: "^links$"
      }
    }
  });

  edits = [
    ...edits,
    ...linkNodes.map(node => {
      // Comment out the links section instead of removing completely
      const linkContent = node.text();
      const commentedLinks = linkContent
        .split('\n')
        .map(line => `# ${line} # deprecated: use networks instead`)
        .join('\n');
      return node.replace(commentedLinks);
    })
  ];

  // Add networks section to services that had links
  const servicesWithLinks = rootNode.findAll({
    rule: {
      kind: "block_mapping_pair",
      inside: {
        kind: "block_mapping_pair",
        has: {
          field: "key", 
          kind: "plain_scalar",
          regex: "^services$"
        }
      },
      has: {
        kind: "block_mapping_pair",
        has: {
          field: "key",
          kind: "plain_scalar", 
          regex: "^links$"
        }
      }
    }
  });

  // For each service with links, add networks configuration
  edits = [
    ...edits,
    ...servicesWithLinks.map(serviceNode => {
      const serviceContent = serviceNode.text();
      // Add networks section to the service
      if (!serviceContent.includes('networks:')) {
        const updatedService = serviceContent + '\n    networks:\n      - default';
        return serviceNode.replace(updatedService);
      }
      return null;
    }).filter(Boolean)
  ];

  if (edits.length === 0) {
    return null;
  }

  const newSource = rootNode.commitEdits(edits as Edit[]);
  return newSource;
}

export default transform;

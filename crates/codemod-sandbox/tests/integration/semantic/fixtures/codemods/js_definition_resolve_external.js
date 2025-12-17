export default function transform(root) {
  // Find the import specifier
  const importSpec = root.root().find({ rule: { pattern: "something" } });
  if (!importSpec) {
    throw new Error("Expected to find 'something'");
  }

  // Get definition with resolveExternal: false
  const def = importSpec.definition({ resolveExternal: false });

  if (!def) {
    throw new Error("Expected to find definition for 'something' (import statement)");
  }

  // Should be an import kind since we're not resolving external
  if (def.kind !== "import") {
    throw new Error("Expected definition kind to be 'import', got '" + def.kind + "'");
  }

  console.log("Definition kind:", def.kind);
  console.log("Definition text:", def.node.text());

  return null;
}

export default function transform(root) {
  // Find the 'add' that's in a call (not the import) and follow it to utils.ts.
  const addNodes = root.root().findAll({ rule: { pattern: "add" } });
  let callAdd = null;
  for (const node of addNodes) {
    const parent = node.parent();
    if (parent && parent.kind() === "call_expression") {
      callAdd = node;
      break;
    }
  }
  if (!callAdd) {
    throw new Error("Could not find add() call");
  }

  // The call resolves to the local import binding first; follow the chain
  // until the definition lives in another file.
  let definition = callAdd.definition();
  for (
    let hop = 0;
    definition && definition.root.filename() === root.filename() && hop < 3;
    hop++
  ) {
    definition = definition.node.definition();
  }
  if (!definition || definition.root.filename() === root.filename()) {
    throw new Error(
      "Expected a definition in another file, got " + (definition && definition.kind),
    );
  }

  // Edit the other file through write(); the host decides whether that hits disk.
  definition.root.write(definition.root.root().text().replace("add", "sum"));
  return null;
}

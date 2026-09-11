// Follows the `add()` call to its definition in another file through the
// shared workspace semantic index, edits that file with `write()`, and
// reports where the definition lives.
interface Node {
  parent(): Node | null;
  kind(): string;
  definition(): { node: Node; root: Root; kind: string } | null;
}
interface Root {
  root(): { text(): string; findAll(rule: unknown): Node[] };
  filename(): string;
  relativeFilename(): string;
  write(content: string): void;
}

export default async function transform(root: Root) {
  const file = root.relativeFilename().replaceAll("\\", "/");
  const call = root
    .root()
    .findAll({ rule: { pattern: "add" } })
    .find((node) => node.parent()?.kind() === "call_expression");
  if (!call) return { content: null, output: { file, definition: null } };
  let definition = call.definition();
  for (
    let hop = 0;
    definition && definition.root.filename() === root.filename() && hop < 3;
    hop++
  ) {
    definition = definition.node.definition();
  }
  if (!definition) return { content: null, output: { file, definition: null } };
  definition.root.write(definition.root.root().text().replace("add", "sum"));
  return {
    content: null,
    output: { file, definition: definition.root.relativeFilename().replaceAll("\\", "/") },
  };
}

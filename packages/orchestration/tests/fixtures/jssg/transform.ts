export default async function transform(root: {
  root(): { text(): string };
  relativeFilename(): string;
}) {
  const content = root.root().text();
  return {
    content: content.replaceAll("oldApi", "newApi"),
    output: { file: root.relativeFilename().replaceAll("\\", "/") },
  };
}

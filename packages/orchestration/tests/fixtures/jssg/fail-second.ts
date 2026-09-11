// Edits every file but throws on `b.ts`, so a command over a.ts and b.ts
// must leave both untouched.
export default async function transform(root: {
  root(): { text(): string };
  relativeFilename(): string;
}) {
  if (root.relativeFilename().endsWith("b.ts")) throw new Error("second file exploded");
  return root.root().text().replaceAll("oldApi", "newApi");
}

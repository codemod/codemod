// Tries to rename the file outside the target root.
export default async function transform(root: {
  root(): { text(): string };
  rename(path: string): void;
}) {
  root.rename("../escaped.ts");
  return root.root().text();
}

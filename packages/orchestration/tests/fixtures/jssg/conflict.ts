// Every file renames itself to the same destination.
export default async function transform(root: {
  root(): { text(): string };
  rename(path: string): void;
}) {
  root.rename("same.ts");
  return root.root().text();
}

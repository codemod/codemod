// Renames `*.old.ts` to `*.new.ts` beside the file and rewrites its content.
export default async function transform(root: {
  root(): { text(): string };
  filename(): string;
  rename(path: string): void;
}) {
  const name = root.filename().split(/[\\/]/).pop()!;
  if (name.endsWith(".old.ts")) root.rename(name.replace(/\.old\.ts$/, ".new.ts"));
  return root.root().text().replaceAll("oldApi", "newApi");
}

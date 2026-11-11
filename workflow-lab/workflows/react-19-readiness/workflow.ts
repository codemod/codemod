/**
 * React 19 readiness: per package of a monorepo, which React 19 breaking
 * APIs it still uses and which React versions it declares. Nothing is written.
 *
 *   codemod workflow run -w workflow.ts --target ~/repos/backstage
 */
import { dynamic, jssg } from "@codemod.com/orchestration";
import { reactDeps, scan, type Category, type Hit } from "./apis.ts";

const manifests = jssg({
  name: "react-deps",
  language: "json",
  include: ["**/package.json"],
  exclude: ["**/node_modules/**"],
  transform(root) {
    const deps = reactDeps(root.root().text());
    return Object.keys(deps).length === 0
      ? null
      : { output: { file: root.relativeFilename(), deps } };
  },
});

const apis = jssg({
  name: "react-19-apis",
  language: "tsx",
  include: ["**/*.tsx", "**/*.ts", "**/*.jsx", "**/*.js"],
  exclude: ["**/node_modules/**", "**/dist/**", "**/*.d.ts"],
  transform(root) {
    const hits = scan(root);
    return hits.length === 0 ? null : { output: { file: root.relativeFilename(), hits } };
  },
});

type ManifestRow = { file: string; deps: Record<string, string> };
type ApiRow = { file: string; hits: Hit[] };

/** `plugins/catalog/src/x.tsx` -> `plugins/catalog`; the package a file belongs to. */
const packageOf = (file: string) => file.split("/").slice(0, 2).join("/");

export default dynamic(async () => {
  const [deps, files] = (await Promise.all([manifests(), apis()])) as [ManifestRow[], ApiRow[]];

  const packages = new Map<
    string,
    {
      name: string;
      deps: Record<string, string>;
      counts: Partial<Record<Category, number>>;
      tests: number;
      files: ApiRow[];
    }
  >();
  const entry = (name: string) => {
    let p = packages.get(name);
    if (!p) packages.set(name, (p = { name, deps: {}, counts: {}, tests: 0, files: [] }));
    return p;
  };
  for (const { file, deps: d } of deps)
    if (file.split("/").length === 3) Object.assign(entry(packageOf(file)).deps, d);
  for (const row of files) {
    const p = entry(packageOf(row.file));
    p.files.push(row);
    for (const hit of row.hits) p.counts[hit.category] = (p.counts[hit.category] ?? 0) + 1;
  }

  const rows = [...packages.values()]
    .map((p) => ({ ...p, total: Object.values(p.counts).reduce((a, b) => a + (b ?? 0), 0) }))
    .filter((p) => p.total > 0 || Object.keys(p.deps).length > 0)
    .sort((a, b) => b.total - a.total || a.name.localeCompare(b.name));

  const totals: Partial<Record<Category, number>> = {};
  for (const p of rows)
    for (const [c, n] of Object.entries(p.counts))
      totals[c as Category] = (totals[c as Category] ?? 0) + (n ?? 0);

  return {
    kind: "react-19-readiness",
    packages: rows,
    totals,
    ready: rows.filter((p) => p.total === 0).length,
    blocked: rows.filter((p) => p.total > 0).length,
  };
});

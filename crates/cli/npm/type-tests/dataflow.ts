// The data-flow and call-graph answers narrow the way a codemod uses them.
import {
  callGraph,
  crawl,
  dataflow,
  entryPoints,
  lookup,
  symbols,
  type EntryPoint,
  type EntryPoints,
  type Mutation,
  type SgNode,
  type SgRoot,
} from "codemod:ast-grep";

declare const root: SgRoot;
declare const node: SgNode;

// Where a value comes from: the sources lookup, walked by crawl.
export async function fromRequest(value: SgNode): Promise<boolean> {
  const origins = await crawl(dataflow("typescript").sources, value);
  return [...origins.keys()].some((n) => n.kind() === "identifier" && n.text() === "req");
}
// One hop forward is a lookup; crawl walks it.
const { uses, sources } = dataflow("typescript");
export const receivers: SgNode[] = (uses.get(node) ?? [])
  .filter((edge) => edge.via === "argument")
  .map((edge) => edge.node);
export const everyUse = crawl(uses, node);
void sources;

// The call graph: resolved calls, with the call site.
const { callees, callers } = callGraph("typescript");
for (const callee of callees.get(node) ?? []) {
  const call: SgNode = callee.site;
  const via: string = callee.via;
  void [call, via];
}
for (const caller of callers.get(node) ?? []) {
  const edit = caller.site.replace(`await ${caller.site.text()}`);
  void edit;
}

// The node methods the lookups replace are gone.
// @ts-expect-error use callGraph(lang).callees
node.callees();
// @ts-expect-error use callGraph(lang).callers
node.callers();
// @ts-expect-error use crawl(dataflow(lang).uses, node)
node.flowsTo();
// @ts-expect-error use types(lang).typeOf
node.typeInfo();
// @ts-expect-error use entryPoints(lang)
node.entryPoints();

// Mutations.
const mutations: Mutation[] = node.mutations({ after: root.root() });
export const definite = mutations.filter(
  (m) => m.confidence === "definite" && !m.viaAlias && !m.deferred,
);

// Definitions and references are lookups on symbols(lang).
const { definition, references } = symbols("typescript");
for (const def of definition.get(node) ?? []) {
  const exportName: string | null = def.exportName;
  const nameText: string | undefined = def.nameNode?.text();
  void [exportName, nameText];
}
export const referenced: SgNode[] = (references.get(node) ?? []).map((ref) => ref.node);

// Identity.
export const sameNode: boolean = node.equals(root.root()) || node.key() === root.root().key();

// The node methods the lookups and crawl replace are gone.
// @ts-expect-error use symbols(lang).definition
node.definition();
// @ts-expect-error use symbols(lang).references
node.references();
// @ts-expect-error use crawl(dataflow(lang).sources, node)
node.flowsFrom();
// @ts-expect-error use crawl(dataflow(lang).uses, node).has(target)
node.reaches(root.root());

// Entry points: each function, with why it is called from outside.
const entries: EntryPoints = entryPoints("typescript");
for (const [fn, reasons] of entries) {
  const why: string = reasons.map((r: EntryPoint) => `${r.kind} ${r.label}`).join(", ");
  void [fn.key(), why];
}
export const reachable = crawl(callGraph("typescript").callees, entries);
export const restored: EntryPoints = lookup(entries.toJSON());
// @ts-expect-error entryPoints is not a lookup to crawl through
crawl(entries, node);

// Module helpers on the root.
// @ts-expect-error moved to symbols(lang).resolveModule
root.resolveModule("./x");
export const effects: boolean = root.hasTopLevelEffects();

// Module resolution on symbols(lang): a path, a workflow file, a root or a node.
const { resolveModule, specifierFor } = symbols("typescript");
export const resolved: string | null = resolveModule(root, "@/ui");
export const fromFile: string | null = resolveModule({ path: "src/app.ts" }, "./lib");
export const specifier: string | null = specifierFor(node, root, { like: "@/ui", prefer: "alias" });
// @ts-expect-error prefer is "relative" or "alias"
specifierFor(root, root, { prefer: "short" });

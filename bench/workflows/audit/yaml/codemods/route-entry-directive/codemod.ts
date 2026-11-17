import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("route-entry-directive");

// App Router entry files: app/**/page.tsx, layout.tsx, template.tsx (also .jsx/.ts/.js).
const ROUTE_ENTRY = /(^|\/)app\/(.+\/)?(page|layout|template)\.[jt]sx?$/;

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\troute-entry-directive\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const entry = root.filename().replace(/\\/g, "/").match(ROUTE_ENTRY)?.[3];
  if (!entry) return null;
  const program = root.root();
  const first = program.children().find((c) => c.isNamed() && c.kind() !== "comment");
  const directive = first?.kind() === "expression_statement" ? first.children().find((c) => c.isNamed())?.text() : "";
  // "use client" pushes the route to the browser; "use server" turns every export,
  // including the page or layout component itself, into a callable server action.
  const name = directive?.replace(/^["']|["']$/g, "");
  if (name === "use client" || name === "use server") report(first!, { entry, directive: name.replace(" ", "-") });
  return null;
};

export default codemod;

import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("exhaustive-deps-suppressed");

const SUPPRESSIONS: [string, RegExp][] = [
  ["eslint", /eslint-disable(-next-line|-line)?\b.*react-hooks\/exhaustive-deps/],
  ["biome", /biome-ignore\b.*useExhaustiveDependencies/],
  ["oxlint", /oxlint-disable(-next-line|-line)?\b.*exhaustive-deps/],
];

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\texhaustive-deps-suppressed\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  for (const comment of root.root().findAll({ rule: { kind: "comment" } })) {
    const tool = SUPPRESSIONS.find(([, re]) => re.test(comment.text()))?.[0];
    // `-- reason` (eslint/oxlint) or `: reason` (biome) records why the deps were left out.
    const justified = /(exhaustive-deps|useExhaustiveDependencies)\s*(--|:)\s*\S/.test(comment.text());
    if (tool) report(comment, { tool, justified: justified ? "yes" : "no" });
  }
  return null;
};

export default codemod;

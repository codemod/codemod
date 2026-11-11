// Generated from the jssg React/Next.js audit (devagrawal09/nextjs-codemods):
// the same detector, reporting findings as step output instead of metrics.
import type { SgNode, SgRoot } from "codemod:ast-grep";
import type { Finding } from "./finding.ts";
import type TSX from "codemod:ast-grep/langs/tsx";

// Demo apps, examples and OG images rendered by Satori aren't shipped UI.
const EXAMPLE_PATH =
  /(^|\/)(examples?|demos?|playgrounds?|sandbox(es)?)\/|[-_.](demo|example|dummy)s?[-_./]/i;
const OG_IMAGE = /from\s+["'](next\/og|@vercel\/og|satori)["']|new ImageResponse\(/;

const SUPPRESSIONS: [string, RegExp][] = [
  ["eslint", /eslint-disable(-next-line|-line)?\b.*react-hooks\/exhaustive-deps/],
  ["biome", /biome-ignore\b.*useExhaustiveDependencies/],
  ["oxlint", /oxlint-disable(-next-line|-line)?\b.*exhaustive-deps/],
];

export const detect = async (
  root: SgRoot<TSX>,
  options: { params?: Record<string, unknown> },
  found: Finding[],
) => {
  // --param includeExamples=1 keeps example and demo code, which is sometimes copied into apps.
  if (
    !options.params?.includeExamples &&
    (EXAMPLE_PATH.test(root.filename().replace(/\\/g, "/")) || OG_IMAGE.test(root.root().text()))
  )
    return null;
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    found.push({ rule: "exhaustive-deps-suppressed", line: node.range().start.line + 1, dims });
  };

  for (const comment of root.root().findAll({ rule: { kind: "comment" } })) {
    const tool = SUPPRESSIONS.find(([, re]) => re.test(comment.text()))?.[0];
    // `-- reason` (eslint/oxlint) or `: reason` (biome) records why the deps were left out.
    const justified = /(exhaustive-deps|useExhaustiveDependencies)\s*(--|:)\s*\S/.test(
      comment.text(),
    );
    if (tool) report(comment, { tool, justified: justified ? "yes" : "no" });
  }
  return null;
};

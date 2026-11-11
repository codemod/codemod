// Generated from the jssg React/Next.js audit (devagrawal09/nextjs-codemods):
// the same detector, reporting findings as step output instead of metrics.
import type { SgNode, SgRoot } from "codemod:ast-grep";
import type { Finding } from "./finding.ts";
import type TSX from "codemod:ast-grep/langs/tsx";

// Demo apps, examples and OG images rendered by Satori aren't shipped UI.
const EXAMPLE_PATH =
  /(^|\/)(examples?|demos?|playgrounds?|sandbox(es)?)\/|[-_.](demo|example|dummy)s?[-_./]/i;
const OG_IMAGE = /from\s+["'](next\/og|@vercel\/og|satori)["']|new ImageResponse\(/;

// A lint suppression right above the element means someone already reviewed it, e.g. a
// clickable header whose inner button carries the keyboard behaviour.
const A11Y_SUPPRESSION = /(eslint|oxlint|biome)[\w-]*.*(jsx-a11y|a11y\/)/;

const STATIC_ELEMENTS =
  "^(div|span|li|td|tr|p|section|article|img|svg|ul|header|footer|main|nav|i|h[1-6])$";

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
    found.push({ rule: "click-on-non-interactive", line: node.range().start.line + 1, dims });
  };

  const elements = root.root().findAll({
    rule: {
      any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
      has: { field: "name", regex: STATIC_ELEMENTS },
    },
  });

  const lines = root.root().text().split("\n");

  for (const element of elements) {
    const own = {
      stopBy: "neighbor" as const,
      kind: element.is("jsx_opening_element")
        ? ("jsx_opening_element" as const)
        : ("jsx_self_closing_element" as const),
    };
    const attributes = element.findAll({ rule: { kind: "jsx_attribute", inside: own } });
    const names = new Set(attributes.map((a) => a.children()[0]?.text() ?? ""));
    if (!names.has("onClick")) continue;
    // A spread may carry role/keyboard props we cannot see.
    if (
      element.has({
        rule: { kind: "jsx_expression", has: { kind: "spread_element" }, inside: own },
      })
    )
      continue;
    if (
      names.has("role") ||
      names.has("onKeyDown") ||
      names.has("onKeyUp") ||
      names.has("onKeyPress")
    )
      continue;

    // onClick={(e) => e.stopPropagation()} only shields a parent; it is not an interaction.
    const handler = attributes.find((a) => a.children()[0]?.text() === "onClick")?.text() ?? "";
    if (
      /^onClick=\{\(?\w*\)?\s*=>\s*\{?\s*\w+\.(stopPropagation|preventDefault)\(\);?\s*\}?\}$/.test(
        handler,
      )
    )
      continue;
    if (/^onClick=\{[\w.]*(stop|prevent)\w*\}$/i.test(handler)) continue;

    const { start, end } = element.range();
    if (
      lines.slice(Math.max(0, start.line - 1), end.line + 1).some((l) => A11Y_SUPPRESSION.test(l))
    )
      continue;

    report(element, { element: element.field("name")?.text() ?? "unknown" });
  }
  return null;
};

import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("click-on-non-interactive");

const STATIC_ELEMENTS = "^(div|span|li|td|tr|p|section|article|img|svg|ul|header|footer|main|nav|i|h[1-6])$";

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\tclick-on-non-interactive\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const elements = root.root().findAll({
    rule: {
      any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
      has: { field: "name", regex: STATIC_ELEMENTS },
    },
  });

  for (const element of elements) {
    const own = { stopBy: "neighbor" as const, kind: element.is("jsx_opening_element") ? ("jsx_opening_element" as const) : ("jsx_self_closing_element" as const) };
    const attributes = element.findAll({ rule: { kind: "jsx_attribute", inside: own } });
    const names = new Set(attributes.map((a) => a.children()[0]?.text() ?? ""));
    if (!names.has("onClick")) continue;
    // A spread may carry role/keyboard props we cannot see.
    if (element.has({ rule: { kind: "jsx_expression", has: { kind: "spread_element" }, inside: own } })) continue;
    if (names.has("role") || names.has("onKeyDown") || names.has("onKeyUp") || names.has("onKeyPress")) continue;

    // onClick={(e) => e.stopPropagation()} only shields a parent; it is not an interaction.
    const handler = attributes.find((a) => a.children()[0]?.text() === "onClick")?.text() ?? "";
    if (/^onClick=\{\(?\w*\)?\s*=>\s*\{?\s*\w+\.(stopPropagation|preventDefault)\(\);?\s*\}?\}$/.test(handler)) continue;

    report(element, { element: element.field("name")?.text() ?? "unknown" });
  }
  return null;
};

export default codemod;

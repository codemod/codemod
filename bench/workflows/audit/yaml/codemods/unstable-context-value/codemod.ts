import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("unstable-context-value");

const FUNCTION_KINDS = new Set(["arrow_function", "function_expression", "function", "function_declaration", "method_definition"]);
const SHAPES: Record<string, string> = {
  object: "object",
  array: "array",
  arrow_function: "function",
  function_expression: "function",
  function: "function",
};

// When the provider wraps `{children}` from props, those children would otherwise skip
// re-rendering; a new value forces every consumer below to re-render anyway.
const wrapsChildrenProp = (attribute: SgNode<TSX>) =>
  attribute.parent()?.parent()?.children().some(
    (c) => c.kind() === "jsx_expression" && /^\{\s*(props\.)?children\s*\}$/.test(c.text()),
  ) ?? false;

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\tunstable-context-value\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const valueAttributes = root.root().findAll({
    rule: {
      kind: "jsx_attribute",
      has: { kind: "property_identifier", regex: "^value$" },
      inside: {
        any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
        has: { field: "name", regex: "(\\.Provider|Context)$" },
        stopBy: "neighbor",
      },
    },
  });

  for (const attribute of valueAttributes) {
    const expression = attribute.find({ rule: { kind: "jsx_expression" } })?.children().find((c) => c.isNamed());
    if (!expression) continue;

    const inlineShape = SHAPES[expression.kind()];
    if (inlineShape) {
      report(attribute, { shape: inlineShape, via: "inline", wraps: wrapsChildrenProp(attribute) ? "children-prop" : "inline-tree" });
      continue;
    }
    if (expression.kind() !== "identifier") continue;

    // `const value = { ... }` declared in the same component body without useMemo.
    const component = attribute.ancestors().find((a) => FUNCTION_KINDS.has(a.kind()));
    const declaration = component?.find({
      rule: { kind: "variable_declarator", has: { field: "name", regex: `^${expression.text()}$` } },
    });
    const localShape = SHAPES[declaration?.field("value")?.kind() ?? ""];
    if (localShape) report(attribute, { shape: localShape, via: "local-variable", wraps: wrapsChildrenProp(attribute) ? "children-prop" : "inline-tree" });
  }
  return null;
};

export default codemod;

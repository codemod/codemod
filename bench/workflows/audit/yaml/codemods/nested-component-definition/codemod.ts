import type { Codemod, Rule, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("nested-component-definition");

const FUNCTION_KINDS = new Set(["arrow_function", "function_expression", "function", "function_declaration", "method_definition"]);
const JSX: Rule<TSX> = { any: [{ kind: "jsx_element" }, { kind: "jsx_self_closing_element" }] };

const owningFunction = (node: SgNode<TSX>) => node.ancestors().find((a) => FUNCTION_KINDS.has(a.kind()));

// `function Row() {}` or `const Row = () => {}`: returns the name node and the function node.
const asComponent = (node: SgNode<TSX>) => {
  if (node.kind() === "function_declaration") {
    const name = node.field("name");
    return name && /^[A-Z]/.test(name.text()) ? { name, fn: node } : null;
  }
  const name = node.field("name");
  const value = node.field("value");
  if (!name || name.kind() !== "identifier" || !/^[A-Z]/.test(name.text()) || !value) return null;
  return value.kind() === "arrow_function" || value.kind() === "function_expression" ? { name, fn: value } : null;
};

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\tnested-component-definition\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const candidates = root.root().findAll({
    rule: { any: [{ kind: "function_declaration" }, { kind: "variable_declarator" }] },
  });

  for (const node of candidates) {
    const inner = asComponent(node);
    if (!inner || !inner.fn.has({ rule: JSX })) continue;
    const outerFn = owningFunction(node);
    if (!outerFn) continue;
    const outerName = outerFn.kind() === "function_declaration" ? outerFn.field("name") : outerFn.parent()?.field("name");
    if (!outerName || !/^[A-Z]/.test(outerName.text())) continue;

    // Only a component rendered as <Inner /> remounts; calling Inner() as a helper does not.
    const name = inner.name.text();
    const renderedAsElement = outerFn.has({
      rule: {
        any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
        has: { field: "name", regex: `^${name}$` },
      },
    });
    if (renderedAsElement) report(node, { declaredAs: node.kind() === "function_declaration" ? "function" : "arrow" });
  }
  return null;
};

export default codemod;

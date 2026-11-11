// Generated from the jssg React/Next.js audit (devagrawal09/nextjs-codemods):
// the same detector, reporting findings as step output instead of metrics.
import type { Rule, SgNode, SgRoot } from "codemod:ast-grep";
import type { Finding } from "./finding.ts";
import type TSX from "codemod:ast-grep/langs/tsx";

// Demo apps, examples and OG images rendered by Satori aren't shipped UI.
const EXAMPLE_PATH =
  /(^|\/)(examples?|demos?|playgrounds?|sandbox(es)?)\/|[-_.](demo|example|dummy)s?[-_./]/i;
const OG_IMAGE = /from\s+["'](next\/og|@vercel\/og|satori)["']|new ImageResponse\(/;

const FUNCTION_KINDS = new Set([
  "arrow_function",
  "function_expression",
  "function",
  "function_declaration",
  "method_definition",
]);
const JSX: Rule<TSX> = { any: [{ kind: "jsx_element" }, { kind: "jsx_self_closing_element" }] };

const ELEMENT_NAME: Rule<TSX> = {
  kind: "identifier",
  inside: {
    any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
    field: "name",
  },
};
const INPUT_ELEMENT =
  /^(input|textarea|select)$|(Input|Textarea|Select|Checkbox|Switch|Radio|Editor|Combobox|Field)$/;
// A remount throws away hook state and re-runs effects and queries; a stateless leaf just re-renders.
const STATEFUL_HOOK =
  "^(React\\.)?use(State|Reducer|Ref|Effect|LayoutEffect|Form|\\w*Query|\\w*Mutation)$";

const owningFunction = (node: SgNode<TSX>) =>
  node.ancestors().find((a) => FUNCTION_KINDS.has(a.kind()));

// `function Row() {}` or `const Row = () => {}`: returns the name node and the function node.
const asComponent = (node: SgNode<TSX>) => {
  if (node.kind() === "function_declaration") {
    const name = node.field("name");
    return name && /^[A-Z]/.test(name.text()) ? { name, fn: node } : null;
  }
  const name = node.field("name");
  const value = node.field("value");
  if (!name || name.kind() !== "identifier" || !/^[A-Z]/.test(name.text()) || !value) return null;
  return value.kind() === "arrow_function" || value.kind() === "function_expression"
    ? { name, fn: value }
    : null;
};

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
    found.push({ rule: "nested-component-definition", line: node.range().start.line + 1, dims });
  };

  const candidates = root.root().findAll({
    rule: { any: [{ kind: "function_declaration" }, { kind: "variable_declarator" }] },
  });

  for (const node of candidates) {
    const inner = asComponent(node);
    if (!inner || !inner.fn.has({ rule: JSX })) continue;
    const outerFn = owningFunction(node);
    if (!outerFn) continue;
    const outerName =
      outerFn.kind() === "function_declaration"
        ? outerFn.field("name")
        : outerFn.parent()?.field("name");
    if (!outerName || !/^[A-Z]/.test(outerName.text())) continue;

    // Only a component rendered as <Inner /> remounts; calling Inner() as a helper does not.
    const name = inner.name.text();
    const renderedAsElement = outerFn.has({
      rule: {
        any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
        has: { field: "name", regex: `^${name}$` },
      },
    });
    if (!renderedAsElement) continue;
    const holdsState = inner.fn.has({
      rule: { kind: "call_expression", has: { field: "function", regex: STATEFUL_HOOK } },
    })
      ? "yes"
      : "no";
    const input = inner.fn.findAll({ rule: ELEMENT_NAME }).some((n) => INPUT_ELEMENT.test(n.text()))
      ? "yes"
      : "no";
    report(node, {
      declaredAs: node.kind() === "function_declaration" ? "function" : "arrow",
      holdsState,
      input,
      harm: holdsState === "yes" || input === "yes" ? "likely" : "unlikely",
    });
  }
  return null;
};

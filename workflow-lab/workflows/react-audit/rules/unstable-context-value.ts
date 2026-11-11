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
  attribute
    .parent()
    ?.parent()
    ?.children()
    .some(
      (c) => c.kind() === "jsx_expression" && /^\{\s*(props\.)?children\s*\}$/.test(c.text()),
    ) ?? false;

// Names the provider owns: its props and its own useState/useReducer values and setters.
const INNER_FUNCTION: Rule<TSX> = {
  any: [{ kind: "arrow_function" }, { kind: "function_expression" }],
};

const ownNames = (component: SgNode<TSX> | undefined) => {
  const names = new Set(["props"]);
  component
    ?.field("parameters")
    ?.findAll({
      rule: { any: [{ kind: "identifier" }, { kind: "shorthand_property_identifier_pattern" }] },
    })
    .forEach((n) => names.add(n.text()));
  component
    ?.findAll({
      rule: {
        kind: "variable_declarator",
        has: { field: "value", regex: "^(React\\.)?use(State|Reducer)\\b" },
      },
    })
    .forEach((d) =>
      d
        .field("name")
        ?.findAll({ rule: { kind: "identifier" } })
        .forEach((n) => names.add(n.text())),
    );
  return names;
};

// A value built only from the provider's own state and props changes whenever the provider re-renders,
// so memoising it saves little; callbacks and derived values are where useMemo/useCallback pay off.
const madeOf = (value: SgNode<TSX>, component: SgNode<TSX> | undefined) => {
  if (value.kind() !== "object" && value.kind() !== "array") return "other";
  const own = ownNames(component);
  const parts = value.children().filter((c) => c.isNamed() && c.kind() !== "comment");
  // Each part may combine the provider's names with literals and ALL_CAPS constants
  // (`flags || {}`, `{ ...config, MODE }`), but a function or a call to anything else is "other".
  const isOwn = (part: SgNode<TSX>): boolean => {
    if (FUNCTION_KINDS.has(part.kind()) || part.find({ rule: INNER_FUNCTION })) return false;
    const names = [
      part,
      ...part.findAll({
        rule: { any: [{ kind: "identifier" }, { kind: "shorthand_property_identifier" }] },
      }),
    ].filter((n) => n.kind() === "identifier" || n.kind() === "shorthand_property_identifier");
    return names.every(
      (n) => own.has(n.text()) || /^[A-Z][A-Z0-9_]*$/.test(n.text()) || n.text() === "undefined",
    );
  };
  // A value of literals alone could be hoisted to a module constant, so it stays "other".
  const usesOwn = value
    .findAll({ rule: { any: [{ kind: "identifier" }, { kind: "shorthand_property_identifier" }] } })
    .some((n) => own.has(n.text()));
  return usesOwn && parts.every(isOwn) ? "own-state-or-props" : "other";
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
    found.push({ rule: "unstable-context-value", line: node.range().start.line + 1, dims });
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
    const expression = attribute
      .find({ rule: { kind: "jsx_expression" } })
      ?.children()
      .find((c) => c.isNamed());
    if (!expression) continue;

    const component = attribute.ancestors().find((a) => FUNCTION_KINDS.has(a.kind()));
    const wraps = wrapsChildrenProp(attribute) ? "children-prop" : "inline-tree";
    const inlineShape = SHAPES[expression.kind()];
    if (inlineShape) {
      report(attribute, {
        shape: inlineShape,
        via: "inline",
        wraps,
        madeOf: madeOf(expression, component),
      });
      continue;
    }
    if (expression.kind() !== "identifier") continue;

    // `const value = { ... }` declared in the same component body without useMemo.
    const declaration = component?.find({
      rule: {
        kind: "variable_declarator",
        has: { field: "name", regex: `^${expression.text()}$` },
      },
    });
    const value = declaration?.field("value");
    const localShape = SHAPES[value?.kind() ?? ""];
    if (value && localShape)
      report(attribute, {
        shape: localShape,
        via: "local-variable",
        wraps,
        madeOf: madeOf(value, component),
      });
  }
  return null;
};

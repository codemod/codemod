// Generated from the jssg React/Next.js audit (devagrawal09/nextjs-codemods):
// the same detector, reporting findings as step output instead of metrics.
import type { Rule, SgNode, SgRoot } from "codemod:ast-grep";
import type { Finding } from "./finding.ts";
import type TSX from "codemod:ast-grep/langs/tsx";

// Demo apps, examples and OG images rendered by Satori aren't shipped UI.
const EXAMPLE_PATH =
  /(^|\/)(examples?|demos?|playgrounds?|sandbox(es)?)\/|[-_.](demo|example|dummy)s?[-_./]/i;
const OG_IMAGE = /from\s+["'](next\/og|@vercel\/og|satori)["']|new ImageResponse\(/;

const CALLBACK_KINDS = new Set(["arrow_function", "function_expression", "function"]);
const FUNCTION_KINDS = new Set([...CALLBACK_KINDS, "function_declaration", "method_definition"]);

// Placeholder lists (`Array.from({ length: 3 })`, `[...Array(n)]`) have nothing but position to key on.
const PLACEHOLDER_SOURCE = /^(\[\s*\.\.\.\s*)?(new\s+)?Array(\.from)?\s*\(/;
const ELEMENT_NAME: Rule<TSX> = {
  kind: "identifier",
  inside: {
    any: [{ kind: "jsx_opening_element" }, { kind: "jsx_self_closing_element" }],
    field: "name",
  },
};
const INPUT_ELEMENT =
  /^(input|textarea|select)$|(Input|Textarea|Select|Checkbox|Switch|Radio|Editor|Combobox|Field)$/;

// The nearest `.map((item, index) => ...)` around the key, when the list has real items.
const mapAround = (node: SgNode<TSX>) => {
  const callback = node.ancestors().find((a) => CALLBACK_KINDS.has(a.kind()));
  const call = callback?.parent()?.parent();
  if (!callback || call?.kind() !== "call_expression") return null;
  const callee = call.field("function");
  const list = callee?.field("object");
  if (
    callee?.kind() !== "member_expression" ||
    !list ||
    !/^(map|flatMap)$/.test(callee.field("property")?.text() ?? "")
  )
    return null;
  if (PLACEHOLDER_SOURCE.test(list.text())) return null;

  const [item, index] =
    callback
      .field("parameters")
      ?.children()
      .filter((c) => c.isNamed()) ?? [];
  const itemName =
    item?.kind() === "identifier"
      ? item.text()
      : item?.find({ rule: { kind: "identifier" } })?.text();
  // `(_, i) =>` or an item that is never read: the list is positional by design.
  if (!itemName || itemName.startsWith("_")) return null;
  const body = callback.field("body");
  if (
    item?.kind() !== "object_pattern" &&
    !body?.has({ rule: { kind: "identifier", regex: `^${itemName}$` } })
  )
    return null;

  const indexName =
    index?.kind() === "identifier" ? index : index?.find({ rule: { kind: "identifier" } });
  return indexName ? { index: indexName.text(), callback, list } : null;
};

// `const COLUMNS = [...]` at module level never changes, so position is a stable key.
const isModuleConstant = (program: SgNode<TSX>, list: SgNode<TSX>) => {
  if (list.kind() !== "identifier") return false;
  const declaration = program.find({
    rule: { kind: "variable_declarator", has: { field: "name", regex: `^${list.text()}$` } },
  });
  return (
    !!declaration &&
    /^\[/.test(declaration.field("value")?.text() ?? "") &&
    !declaration.ancestors().some((a) => FUNCTION_KINDS.has(a.kind()))
  );
};

// Index keys only misbehave when the list changes; these are the signs it does.
const listChange = (program: SgNode<TSX>, list: SgNode<TSX>, callback: SgNode<TSX>) => {
  if (
    callback.has({
      rule: {
        any: [{ kind: "identifier" }, { kind: "property_identifier" }],
        regex: "(?i)^(on|handle)?(remove|delete|move|reorder|drag|swap|insert)",
      },
    })
  )
    return "editable";
  const source = list.text().match(/^[\w$]+/)?.[0] ?? "";
  const fromState = program.has({
    rule: {
      kind: "variable_declarator",
      has: { field: "name", regex: `^[\\[{]\\s*${source}\\b` },
      all: [{ has: { field: "value", regex: "^(React\\.)?use(State|Reducer|FieldArray)\\b" } }],
    },
  });
  if (fromState) return "state";
  return /\.(filter|sort|toSorted|reverse|slice)\(/.test(list.text())
    ? "filtered-or-sorted"
    : "fixed-or-unknown";
};

// What a reused element can carry over: component state, or the DOM state of an input.
const itemKind = (program: SgNode<TSX>, callback: SgNode<TSX>) => {
  const components = callback
    .findAll({ rule: ELEMENT_NAME })
    .map((n) => n.text())
    .filter((n) => /^[A-Z]/.test(n) && n !== "Fragment");
  if (components.length === 0) return "host-elements";
  const stateful = components.some((name) =>
    program.has({
      rule: {
        any: [{ kind: "function_declaration" }, { kind: "variable_declarator" }],
        has: { field: "name", regex: `^${name}$` },
        all: [
          {
            has: {
              kind: "call_expression",
              regex: "^(React\\.)?use(State|Reducer|Ref)\\b",
              stopBy: "end",
            },
          },
        ],
      },
    }),
  );
  return stateful ? "stateful-component" : "component";
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
    found.push({ rule: "index-as-key", line: node.range().start.line + 1, dims });
  };

  const program = root.root();
  const keys = program.findAll({
    rule: { kind: "jsx_attribute", has: { kind: "property_identifier", regex: "^key$" } },
  });

  for (const key of keys) {
    const expression = key
      .find({ rule: { kind: "jsx_expression" } })
      ?.children()
      .find((c) => c.isNamed());
    if (!expression) continue;
    const map = mapAround(key);
    if (!map || isModuleConstant(program, map.list)) continue;
    const identifiers = (
      expression.kind() === "identifier"
        ? [expression]
        : expression.findAll({ rule: { kind: "identifier" } })
    )
      .map((i) => i.text())
      .filter((name) => name !== "String");
    if (identifiers.length === 0 || !identifiers.every((name) => name === map.index)) continue;

    const list = listChange(program, map.list, map.callback);
    const item = itemKind(program, map.callback);
    const input = map.callback
      .findAll({ rule: ELEMENT_NAME })
      .some((n) => INPUT_ELEMENT.test(n.text()))
      ? "yes"
      : "no";
    const changes = list !== "fixed-or-unknown";
    const harm =
      changes && (input === "yes" || item !== "host-elements")
        ? "likely"
        : changes || input === "yes" || item === "stateful-component"
          ? "possible"
          : "unlikely";
    report(key, {
      form: expression.kind() === "identifier" ? "bare-index" : "index-in-expression",
      list,
      item,
      input,
      harm,
    });
  }
  return null;
};

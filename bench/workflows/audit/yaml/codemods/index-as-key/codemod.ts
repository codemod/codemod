import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("index-as-key");

const CALLBACK_KINDS = new Set(["arrow_function", "function_expression", "function"]);

// Placeholder lists (`Array.from({ length: 3 })`, `[...Array(n)]`) have nothing but position to key on.
const PLACEHOLDER_SOURCE = /^(\[\s*\.\.\.\s*)?(new\s+)?Array(\.from)?\s*\(/;

// The index parameter of the nearest `.map((item, index) => ...)` around the key, when the list has real items.
const mapIndexParam = (node: SgNode<TSX>) => {
  const callback = node.ancestors().find((a) => CALLBACK_KINDS.has(a.kind()));
  const call = callback?.parent()?.parent();
  if (!callback || call?.kind() !== "call_expression") return null;
  const callee = call.field("function");
  if (callee?.kind() !== "member_expression" || !/^(map|flatMap)$/.test(callee.field("property")?.text() ?? "")) return null;
  if (PLACEHOLDER_SOURCE.test(callee.field("object")?.text() ?? "")) return null;

  const [item, index] = callback.field("parameters")?.children().filter((c) => c.isNamed()) ?? [];
  const itemName = item?.kind() === "identifier" ? item.text() : item?.find({ rule: { kind: "identifier" } })?.text();
  // `(_, i) =>` or an item that is never read: the list is positional by design.
  if (!itemName || itemName.startsWith("_")) return null;
  const body = callback.field("body");
  if (item?.kind() !== "object_pattern" && !body?.has({ rule: { kind: "identifier", regex: `^${itemName}$` } })) return null;

  const indexName = index?.kind() === "identifier" ? index : index?.find({ rule: { kind: "identifier" } });
  return indexName?.text() ?? null;
};

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\tindex-as-key\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const keys = root.root().findAll({
    rule: { kind: "jsx_attribute", has: { kind: "property_identifier", regex: "^key$" } },
  });

  for (const key of keys) {
    const expression = key.find({ rule: { kind: "jsx_expression" } })?.children().find((c) => c.isNamed());
    if (!expression) continue;
    const index = mapIndexParam(key);
    if (!index) continue;
    const identifiers = (expression.kind() === "identifier" ? [expression] : expression.findAll({ rule: { kind: "identifier" } }))
      .map((i) => i.text())
      .filter((name) => name !== "String");
    if (identifiers.length > 0 && identifiers.every((name) => name === index)) {
      report(key, { form: expression.kind() === "identifier" ? "bare-index" : "index-in-expression" });
    }
  }
  return null;
};

export default codemod;

import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("unsanitized-inner-html");

// Sanitizers, "safe" HTML helpers, and the `.replace(/</g, "\\u003c")` JSON-in-script escape.
const SANITIZER = /sanitiz|purif|xss|escape|safe|\\u003c|\\x3c|&lt;/i;
const MARKDOWN = /marked|markdown|remark|rehype|md|mdx|highlight|shiki|codeToHtml|toHtml|render/i;

const classify = (value: SgNode<TSX>) => {
  if (value.kind() === "call_expression") {
    const callee = value.field("function")?.text() ?? "";
    if (callee === "JSON.stringify") return "json-stringify";
    return MARKDOWN.test(callee) ? "rendered-markup" : "function-result";
  }
  if (value.kind() === "template_string") return "template";
  return value.kind() === "identifier" || value.kind() === "member_expression" ? "variable" : "expression";
};

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\tunsanitized-inner-html\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const attributes = root.root().findAll({
    rule: { kind: "jsx_attribute", has: { kind: "property_identifier", regex: "^dangerouslySetInnerHTML$" } },
  });

  for (const attribute of attributes) {
    // <style> injection is limited to CSS; script and markup sinks are the XSS risk.
    if (attribute.parent()?.field("name")?.text() === "style") continue;
    const html = attribute.find({
      rule: { kind: "pair", has: { field: "key", regex: "^__html$" } },
    })?.field("value");
    if (!html) continue;
    if (html.kind() === "string") continue;
    if (html.kind() === "template_string" && !html.has({ rule: { kind: "template_substitution" } })) continue;
    if (SANITIZER.test(html.text())) continue;
    // `const clean = DOMPurify.sanitize(raw)` earlier in the file counts as sanitized.
    if (html.kind() === "identifier") {
      const declaration = root.root().find({
        rule: { kind: "variable_declarator", has: { field: "name", regex: `^${html.text()}$` } },
      });
      if (declaration && SANITIZER.test(declaration.field("value")?.text() ?? "")) continue;
    }
    report(attribute, { source: classify(html) });
  }
  return null;
};

export default codemod;

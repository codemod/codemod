import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("derived-state-effect");

const LITERAL_KINDS = new Set(["true", "false", "null", "undefined", "number", "string", "array", "object"]);

const isSetterCall = (statement: SgNode<TSX>) => {
  if (statement.kind() !== "expression_statement") return false;
  const call = statement.children().find((c) => c.isNamed());
  if (call?.kind() !== "call_expression") return false;
  if (!/^set[A-Z]/.test(call.field("function")?.text() ?? "")) return false;
  return !call.has({ rule: { any: [{ kind: "await_expression" }, { kind: "arrow_function" }, { kind: "function_expression" }] } });
};

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\tderived-state-effect\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const effects = root.root().findAll({
    rule: { kind: "call_expression", has: { field: "function", regex: "^(React\\.)?useEffect$" } },
  });

  for (const effect of effects) {
    const [callback, deps] = effect.field("arguments")?.children().filter((c) => c.isNamed()) ?? [];
    // An effect with no deps array or `[]` runs on mount, not when an input changes.
    if (!callback || deps?.kind() !== "array" || !deps.children().some((c) => c.isNamed())) continue;
    const body = callback.field("body");
    if (body?.kind() !== "statement_block") continue;

    const statements = body.children().filter((c) => c.isNamed() && c.kind() !== "comment");
    if (statements.length === 0 || statements.length > 3 || !statements.every(isSetterCall)) continue;

    const setterArgs = statements.flatMap(
      (s) => s.find({ rule: { kind: "arguments" } })?.children().filter((c) => c.isNamed()) ?? [],
    );
    const kind = setterArgs.every((a) => LITERAL_KINDS.has(a.kind())) ? "reset-on-change" : "derived-value";
    report(effect, { kind });
  }
  return null;
};

export default codemod;

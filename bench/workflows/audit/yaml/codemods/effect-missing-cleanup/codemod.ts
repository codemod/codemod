import type { Codemod, SgNode } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { useMetricAtom } from "codemod:metrics";

const findings = useMetricAtom("effect-missing-cleanup");

const FUNCTION_KINDS = new Set([
  "arrow_function",
  "function_expression",
  "function",
  "function_declaration",
  "method_definition",
]);

// Each resource needs a matching teardown; `{ once: true }` and AbortSignal
// listeners tear themselves down, so they are not reported.
const RESOURCES: { resource: string; rule: object }[] = [
  {
    resource: "event-listener",
    rule: {
      pattern: "$TARGET.addEventListener($$$ARGS)",
      not: { has: { stopBy: "end", regex: "^(once|signal)$", kind: "property_identifier" } },
    },
  },
  { resource: "interval", rule: { pattern: "setInterval($$$ARGS)" } },
  { resource: "observer", rule: { pattern: "new $OBS($$$ARGS)", has: { field: "constructor", regex: "^(Resize|Intersection|Mutation|Performance)Observer$" } } },
  { resource: "subscription", rule: { any: [{ pattern: "$SOURCE.subscribe($$$ARGS)" }, { pattern: "$SOURCE.on($EVENT, $HANDLER)" }] } },
];

const owningFunction = (node: SgNode<TSX>) =>
  node.ancestors().find((a) => FUNCTION_KINDS.has(a.kind()));

const codemod: Codemod<TSX> = async (root, options) => {
  const report = (node: SgNode<TSX>, dims: Record<string, string>) => {
    findings.increment(dims);
    if (options.params?.locations) {
      console.log(`FINDING\teffect-missing-cleanup\t${root.filename()}:${node.range().start.line + 1}\t${JSON.stringify(dims)}`);
    }
  };

  const effects = root.root().findAll({
    rule: {
      kind: "call_expression",
      has: { field: "function", regex: "^(React\\.)?use(Layout)?Effect$" },
    },
  });

  for (const effect of effects) {
    const callback = effect.field("arguments")?.children().find((c) => c.isNamed());
    if (!callback || !FUNCTION_KINDS.has(callback.kind())) continue;
    const body = callback.field("body");
    if (!body || body.kind() !== "statement_block") continue;

    const returnsCleanup = body
      .findAll({ rule: { kind: "return_statement" } })
      .some((ret) => owningFunction(ret)?.id() === callback.id() && ret.children().some((c) => c.isNamed()));
    if (returnsCleanup) continue;

    for (const { resource, rule } of RESOURCES) {
      const acquired = body.find({ rule });
      if (acquired) {
        report(acquired, { resource });
        break;
      }
    }
  }
  return null;
};

export default codemod;

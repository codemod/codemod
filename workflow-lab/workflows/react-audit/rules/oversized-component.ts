// Generated from the jssg React/Next.js audit (devagrawal09/nextjs-codemods):
// the same detector, reporting findings as step output instead of metrics.
import type { Rule, SgNode, SgRoot } from "codemod:ast-grep";
import type { Finding } from "./finding.ts";
import type TSX from "codemod:ast-grep/langs/tsx";

// Demo apps, examples and OG images rendered by Satori aren't shipped UI.
const EXAMPLE_PATH =
  /(^|\/)(examples?|demos?|playgrounds?|sandbox(es)?)\/|[-_.](demo|example|dummy)s?[-_./]/i;
const OG_IMAGE = /from\s+["'](next\/og|@vercel\/og|satori)["']|new ImageResponse\(/;

const LIMIT = 300;
const JSX: Rule<TSX> = { any: [{ kind: "jsx_element" }, { kind: "jsx_self_closing_element" }] };

const bucket = (lines: number) => (lines >= 1000 ? "1000+" : lines >= 500 ? "500-999" : "300-499");
// Long-but-static components (icon maps, marketing copy) hold little state; this separates them.
const stateBucket = (count: number) =>
  count === 0 ? "0" : count <= 5 ? "1-5" : count <= 10 ? "6-10" : "11+";

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
    found.push({ rule: "oversized-component", line: node.range().start.line + 1, dims });
  };

  // function Foo() {}, const Foo = () => {}, const Foo = forwardRef(() => {}) / memo(...) / observer(...)
  const components = root.root().findAll({
    rule: {
      any: [
        { kind: "function_declaration", has: { field: "name", regex: "^[A-Z]" } },
        {
          kind: "variable_declarator",
          has: { field: "name", kind: "identifier", regex: "^[A-Z]" },
          any: [
            {
              has: {
                field: "value",
                any: [{ kind: "arrow_function" }, { kind: "function_expression" }],
              },
            },
            {
              has: {
                field: "value",
                kind: "call_expression",
                has: {
                  field: "arguments",
                  has: { any: [{ kind: "arrow_function" }, { kind: "function_expression" }] },
                },
              },
            },
          ],
        },
      ],
    },
  });

  for (const component of components) {
    const { start, end } = component.range();
    const lines = end.line - start.line + 1;
    if (lines < LIMIT || !component.has({ rule: JSX })) continue;
    const stateHooks = component.findAll({
      rule: {
        pattern: "$HOOK($$$ARGS)",
        has: { field: "function", regex: "^(React\\.)?use(State|Reducer)$" },
      },
    }).length;
    report(component, { size: bucket(lines), stateHooks: stateBucket(stateHooks) });
  }
  return null;
};

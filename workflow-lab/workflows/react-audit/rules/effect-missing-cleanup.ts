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

// Each resource needs a matching teardown; `{ once: true }` and AbortSignal
// listeners tear themselves down, so they are not reported. A listener or
// subscription removed by its own handler counts as torn down; an interval that
// only clears itself on some later tick keeps running after unmount until then.
const RESOURCES: { resource: string; rule: object; teardown: RegExp; selfRemoves?: boolean }[] = [
  {
    resource: "event-listener",
    rule: {
      pattern: "$TARGET.addEventListener($$$ARGS)",
      not: { has: { stopBy: "end", regex: "^(once|signal)$", kind: "property_identifier" } },
    },
    teardown: /\bremoveEventListener\(/,
    selfRemoves: true,
  },
  {
    resource: "interval",
    rule: { pattern: "setInterval($$$ARGS)" },
    teardown: /\bclearInterval\(/,
  },
  {
    resource: "observer",
    rule: {
      pattern: "new $OBS($$$ARGS)",
      has: { field: "constructor", regex: "^(Resize|Intersection|Mutation|Performance)Observer$" },
    },
    teardown: /\.(disconnect|unobserve)\(/,
  },
  {
    resource: "subscription",
    rule: {
      any: [{ pattern: "$SOURCE.subscribe($$$ARGS)" }, { pattern: "$SOURCE.on($EVENT, $HANDLER)" }],
    },
    teardown: /\b(unsubscribe|off|removeListener|removeChannel|dispose)\(/,
    selfRemoves: true,
  },
];

const EFFECT: Rule<TSX> = {
  kind: "call_expression",
  has: { field: "function", regex: "^(React\\.)?use(Layout)?Effect$" },
};

const owningFunction = (node: SgNode<TSX>) =>
  node.ancestors().find((a) => FUNCTION_KINDS.has(a.kind()));

const callbackOf = (effect: SgNode<TSX>) => {
  const callback = effect
    .field("arguments")
    ?.children()
    .find((c) => c.isNamed());
  return callback && FUNCTION_KINDS.has(callback.kind()) ? callback : null;
};

// The function an effect returns: `return () => ...` in a block body, or `() => () => ...`.
const cleanupOf = (callback: SgNode<TSX>) => {
  const body = callback.field("body");
  if (body && FUNCTION_KINDS.has(body.kind())) return body;
  return (
    body
      ?.findAll({ rule: { kind: "return_statement" } })
      .find(
        (ret) =>
          owningFunction(ret)?.id() === callback.id() && ret.children().some((c) => c.isNamed()),
      ) ?? null
  );
};

// Body text without functions that are written as bare statements and never run,
// like a cleanup arrow that is missing its `return`.
const liveText = (body: SgNode<TSX>) =>
  body
    .findAll({ rule: { kind: "expression_statement", has: { kind: "arrow_function" } } })
    .filter((s) =>
      s
        .children()
        .filter((c) => c.isNamed())
        .every((c) => FUNCTION_KINDS.has(c.kind())),
    )
    .reduce((text, dead) => text.replace(dead.text(), ""), body.text());

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
    found.push({ rule: "effect-missing-cleanup", line: node.range().start.line + 1, dims });
  };

  for (const effect of root.root().findAll({ rule: EFFECT })) {
    const callback = callbackOf(effect);
    const body = callback?.field("body");
    if (!callback || body?.kind() !== "statement_block" || cleanupOf(callback)) continue;

    // Teardown in a separate unmount effect, `useEffect(() => () => ..., [])`, in the same component counts.
    const component = owningFunction(effect);
    const siblingCleanups = (component?.findAll({ rule: EFFECT }) ?? [])
      .filter(
        (other) =>
          other.id() !== effect.id() &&
          /^\[\s*\]$/.test(
            other
              .field("arguments")
              ?.children()
              .filter((c) => c.isNamed())[1]
              ?.text() ?? "",
          ),
      )
      .map((other) => cleanupOf(callbackOf(other) ?? other)?.text() ?? "")
      .join("\n");

    for (const { resource, rule, teardown, selfRemoves } of RESOURCES) {
      const acquired = body.find({ rule });
      if (!acquired) continue;
      const removedInBody = selfRemoves && teardown.test(liveText(body));
      if (!removedInBody && !teardown.test(siblingCleanups)) report(acquired, { resource });
      break;
    }
  }
  return null;
};

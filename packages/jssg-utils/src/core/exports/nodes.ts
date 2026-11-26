import type { SgNode } from "codemod/main";

// Node kinds by role, for the languages the semantic queries cover
// (JavaScript, TypeScript, TSX, Python). A kind no table lists is not matched.

const FUNCTIONS = new Set([
  "function_declaration",
  "function_expression",
  "generator_function_declaration",
  "generator_function",
  "arrow_function",
  "method_definition",
  "function_definition", // Python
  "lambda", // Python
]);

const CLASSES = new Set([
  "class_declaration",
  "class",
  "abstract_class_declaration",
  "class_definition",
]);

const LOOPS = new Set([
  "for_statement",
  "for_in_statement",
  "while_statement",
  "do_statement",
  // Python comprehensions: the loop is the comprehension, its clause holds the target.
  "list_comprehension",
  "set_comprehension",
  "dictionary_comprehension",
  "generator_expression",
]);

/** Array methods whose callback runs once per element. */
const ITERATING_METHODS = new Set([
  "map",
  "forEach",
  "flatMap",
  "filter",
  "some",
  "every",
  "find",
  "findIndex",
  "reduce",
  "reduceRight",
]);

/** Where a statement can sit: a block of statements, a module. */
const STATEMENT_PARENTS = new Set([
  "program",
  "statement_block",
  "module",
  "block",
  "switch_case",
  "class_body",
]);

/** The nearest ancestor (not the node itself) that matches. */
function nearest(node: SgNode, test: (n: SgNode) => boolean): SgNode | null {
  return node.ancestors().find(test) ?? null;
}

/**
 * The function the node is in: a declaration, expression, arrow function or
 * method (JS/TS), or a `def` / `lambda` (Python). Null at module level.
 *
 * @example
 * ```ts
 * const fn = enclosingFunction(call);
 * const refs = symbols("typescript").references.get(name) ?? [];
 * const own = refs.map((r) => r.node).filter((n) => fn?.contains(n));
 * ```
 */
export function enclosingFunction(node: SgNode): SgNode | null {
  return nearest(node, (n) => FUNCTIONS.has(String(n.kind())));
}

/** The class the node is in (`class` declaration or expression; Python `class`), or null. */
export function enclosingClass(node: SgNode): SgNode | null {
  return nearest(node, (n) => CLASSES.has(String(n.kind())));
}

/**
 * The statement the node is part of: the node itself or its nearest
 * ancestor whose parent is a block, a module or a class body. For a name in
 * `export const a = f()`, the `export` statement.
 */
export function enclosingStatement(node: SgNode): SgNode | null {
  for (const candidate of [node, ...node.ancestors()]) {
    const parent = candidate.parent();
    if (parent && STATEMENT_PARENTS.has(String(parent.kind()))) return candidate;
  }
  return null;
}

/** A loop around a node, with its parts. */
export type Loop = {
  /** The loop statement, comprehension, or the iterating call (`items.map(…)`). */
  node: SgNode;
  kind: "for-of" | "for-in" | "for" | "while" | "do" | "callback" | "comprehension";
  /**
   * What each iteration binds: the `for…of` / `for…in` / Python `for` target,
   * or the callback's first parameter. Empty for `for (;;)`, `while`, `do`.
   */
  item: SgNode | null;
  /** What is iterated: `for (x of items)` → `items`, `items.map(cb)` → `items`. */
  iterable: SgNode | null;
  /** The body that runs per iteration (the callback for a `callback` loop). */
  body: SgNode | null;
};

function describeLoop(n: SgNode): Loop | null {
  const kind = String(n.kind());
  if (kind === "for_in_statement") {
    // JS for…of / for…in share a node kind; Python's `for` statement is for_statement.
    const operator =
      n.field("operator")?.text() ??
      (/^for\s*(await\s*)?\([^)]*\sof\s/.test(n.text()) ? "of" : "in");
    return {
      node: n,
      kind: operator === "of" ? "for-of" : "for-in",
      item: n.field("left"),
      iterable: n.field("right"),
      body: n.field("body"),
    };
  }
  if (kind === "for_statement") {
    // Python: `for x in xs:`; JS: `for (init; cond; step)`.
    const python = !!n.field("left") && !!n.field("right");
    return python
      ? {
          node: n,
          kind: "for-of",
          item: n.field("left"),
          iterable: n.field("right"),
          body: n.field("body"),
        }
      : { node: n, kind: "for", item: null, iterable: null, body: n.field("body") };
  }
  if (kind === "while_statement")
    return { node: n, kind: "while", item: null, iterable: null, body: n.field("body") };
  if (kind === "do_statement")
    return { node: n, kind: "do", item: null, iterable: null, body: n.field("body") };
  if (LOOPS.has(kind)) {
    const clause = n.find({ rule: { kind: "for_in_clause" } });
    return {
      node: n,
      kind: "comprehension",
      item: clause?.field("left") ?? null,
      iterable: clause?.field("right") ?? null,
      body: n.field("body"),
    };
  }
  return null;
}

/** `items.map((x) => …)`: the callback's loop, when `fn` is such a callback. */
function callbackLoop(fn: SgNode): Loop | null {
  const args = fn.parent();
  const call = args?.parent();
  if (!args?.is("arguments") || !call?.is("call_expression")) return null;
  const callee = call.field("function");
  if (!callee?.is("member_expression")) return null;
  const method = callee.field("property")?.text() ?? "";
  if (!ITERATING_METHODS.has(method)) return null;
  // reduce's callback gets the element second.
  const parameters =
    fn
      .field("parameters")
      ?.children()
      .filter((c) => c.isNamed()) ?? [];
  const single = fn.field("parameter");
  const item = method.startsWith("reduce")
    ? (parameters[1] ?? null)
    : (single ?? parameters[0] ?? null);
  return { node: call, kind: "callback", item, iterable: callee.field("object"), body: fn };
}

/**
 * The innermost loop the node runs in: `for`, `for…of`, `for…in`, `while`,
 * `do`, an iterating callback (`items.map(…)`, `forEach`, `filter`, …), or a
 * Python `for` / comprehension. Stops at a function boundary that is not an
 * iterating callback: a function declared in a loop does not run per
 * iteration. Null outside any loop.
 *
 * @example
 * ```ts
 * // N+1: a query made once per element
 * const loop = enclosingLoop(call);
 * const origins = await crawl(dataflow("typescript").sources, arg);
 * if (loop?.item && origins.has(loop.item)) report(call, loop.iterable);
 * ```
 */
export function enclosingLoop(node: SgNode): Loop | null {
  for (const ancestor of node.ancestors()) {
    if (FUNCTIONS.has(String(ancestor.kind()))) {
      const loop = callbackLoop(ancestor);
      if (loop) return loop;
      return null;
    }
    const loop = describeLoop(ancestor);
    // A node in the loop's iterable or header is not inside the loop body.
    if (loop && (!loop.body || loop.body.contains(node) || loop.kind === "comprehension"))
      return loop;
  }
  return null;
}

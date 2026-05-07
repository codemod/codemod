import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { SgNode } from "@codemod.com/jssg-types/main";

type Language = JS | TS | TSX;

type EffectiveParentContext = {
  node: SgNode<Language>;
  parent: SgNode<Language> | null;
};

type FieldName = Parameters<SgNode<Language>["field"]>[0];

/**
 * Returns the named, non-comment children of `node`.
 *
 * This is useful when walking statement containers such as blocks or program
 * bodies without having to filter out punctuation and comment trivia manually.
 *
 * @param node The AST node whose direct children should be inspected.
 * @returns The direct named children of `node`, excluding comment nodes.
 */
export function getNamedChildren(node: SgNode<Language>): SgNode<Language>[] {
  return node
    ? node
        .children()
        .filter((child): child is SgNode<Language> => child.isNamed() && child.kind() !== "comment")
    : [];
}

function getFieldNode(node: SgNode<Language>, name: FieldName) {
  return node?.field(name);
}

function getFieldText(node: SgNode<Language>, name: FieldName): string | null {
  return getFieldNode(node, name)?.text() ?? null;
}

function getStaticStringKey(node: SgNode<Language>): string | null {
  if (!node) {
    return null;
  }

  if (node.is("string")) {
    return node
      .children()
      .slice(1, -1)
      .map((child) => child.text())
      .join("");
  }

  if (
    node.is("template_string") &&
    !node.children().some((child) => child.isNamed() && child.kind() === "template_substitution")
  ) {
    return node
      .children()
      .slice(1, -1)
      .map((child) => child.text())
      .join("");
  }

  return null;
}

function shouldBubbleTransparentWrapper(
  parent: SgNode<Language>,
  current: SgNode<Language>,
  includeAssignmentRhs: boolean,
): boolean {
  if (parent.kind() === "parenthesized_expression") {
    return true;
  }

  if (parent.kind() === "sequence_expression") {
    const namedChildren = getNamedChildren(parent);
    return namedChildren[namedChildren.length - 1]?.id() === current.id();
  }

  if (parent.kind() === "ternary_expression") {
    return (
      getFieldNode(parent, "consequence")?.id() === current.id() ||
      getFieldNode(parent, "alternative")?.id() === current.id()
    );
  }

  if (parent.kind() === "binary_expression") {
    const operator = getFieldText(parent, "operator");
    if (operator === "&&" || operator === "||" || operator === "??") {
      return getFieldNode(parent, "right")?.id() === current.id();
    }
  }

  if (includeAssignmentRhs && parent.kind() === "assignment_expression") {
    return getFieldNode(parent, "right")?.id() === current.id();
  }

  return false;
}

function unwrapTransparentWrappersInternal(
  node: SgNode<Language>,
  includeAssignmentRhs: boolean,
): SgNode<Language> {
  let current = node;
  let parent = current.parent();

  while (parent && shouldBubbleTransparentWrapper(parent, current, includeAssignmentRhs)) {
    current = parent;
    parent = current.parent();
  }

  return current;
}

function unwrapTransparentWrappers(node: SgNode<Language>): SgNode<Language> {
  return unwrapTransparentWrappersInternal(node, true);
}

/**
 * Removes only nested parenthesized-expression wrappers from `node`.
 *
 * Unlike the broader transparent-wrapper logic used internally, this helper does
 * not bubble through sequence, ternary, or assignment contexts.
 *
 * @param node The node to unwrap.
 * @returns The innermost non-parenthesized node reachable by stripping `(...)`.
 */
export function unwrapParenthesizedExpression(node: SgNode<Language>): SgNode<Language> {
  let current = node;

  while (current.kind() === "parenthesized_expression") {
    const inner = getNamedChildren(current)[0];
    if (!inner) {
      break;
    }

    current = inner;
  }
  return current;
}

function findEffectiveParentContext(node: SgNode<Language>): EffectiveParentContext {
  const effectiveNode = unwrapTransparentWrappers(node);
  return {
    node: effectiveNode,
    parent: effectiveNode.parent(),
  };
}

/**
 * Returns `true` when `node` is used as the constructor target of a `new`
 * expression, even through transparent wrappers such as parentheses or trailing
 * sequence-expression positions.
 *
 * @param node The expression node to inspect.
 * @returns `true` when the effective usage is `new node(...)`; otherwise `false`.
 */
export function isUsedAsConstructor(node: SgNode<Language>): boolean {
  const { node: effectiveNode, parent } = findEffectiveParentContext(node);
  return (
    parent?.kind() === "new_expression" &&
    getFieldNode(parent, "constructor")?.id() === effectiveNode.id()
  );
}

/**
 * Returns `true` when `node` is used in reflective property access for one of
 * the provided `keys`.
 *
 * Supported reflective forms include:
 * - `node.name`
 * - `node["name"]`
 * - ``node[`name`]``
 * - `"name" in node`
 *
 * Dynamic computed keys are intentionally rejected.
 *
 * @param node The expression node to inspect.
 * @param keys The reflective property names to treat as significant.
 * @returns `true` when `node` participates in one of the supported reflective
 * access patterns for a requested key; otherwise `false`.
 */
export function isUsedInReflectiveAccess(node: SgNode<Language>, keys: string[] = []): boolean {
  const keySet = new Set(keys);
  const { node: effectiveNode, parent } = findEffectiveParentContext(node);

  if (!parent) {
    return false;
  }

  if (
    parent.kind() === "member_expression" &&
    getFieldNode(parent, "object")?.id() === effectiveNode.id()
  ) {
    const property = getFieldText(parent, "property") ?? "";
    return keySet.has(property);
  }

  if (
    parent.kind() === "subscript_expression" &&
    getFieldNode(parent, "object")?.id() === effectiveNode.id()
  ) {
    const fieldNode = getFieldNode(parent, "index");
    if (!fieldNode) return false;
    const key = getStaticStringKey(fieldNode);
    return key ? keySet.has(key) : false;
  }

  if (
    parent.kind() === "binary_expression" &&
    getFieldNode(parent, "right")?.id() === effectiveNode.id() &&
    getFieldText(parent, "operator") === "in"
  ) {
    const fieldNode = getFieldNode(parent, "left");
    if (!fieldNode) return false;
    const key = getStaticStringKey(fieldNode);
    return key ? keySet.has(key) : false;
  }

  return false;
}

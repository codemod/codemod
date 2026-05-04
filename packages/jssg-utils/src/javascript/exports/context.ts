import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { SgNode } from "@codemod.com/jssg-types/main";

type Language = JS | TS | TSX;
type AnyNode = SgNode<Language>;

type EffectiveParentContext = {
  node: AnyNode;
  parent: AnyNode | null;
};

function getNamedChildren(node: AnyNode | null): AnyNode[] {
  return node
    ? node
        .children()
        .filter((child): child is AnyNode => child.isNamed() && child.kind() !== "comment")
    : [];
}

function getFieldNode(node: AnyNode | null, name: string): AnyNode | null {
  return (node?.field(name as never) as AnyNode | null | undefined) ?? null;
}

function getFieldText(node: AnyNode | null, name: string): string | null {
  return getFieldNode(node, name)?.text() ?? null;
}

function getStaticStringKey(node: AnyNode | null): string | null {
  if (!node) {
    return null;
  }

  const text = node.text();
  if (
    (text.startsWith("'") && text.endsWith("'")) ||
    (text.startsWith('"') && text.endsWith('"')) ||
    (text.startsWith("`") && text.endsWith("`") && !text.includes("${"))
  ) {
    return text.slice(1, -1);
  }

  return null;
}

function shouldBubbleTransparentWrapper(
  parent: AnyNode,
  current: AnyNode,
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

function unwrapTransparentWrappersInternal(node: AnyNode, includeAssignmentRhs: boolean): AnyNode {
  let current = node;
  let parent = current.parent() as AnyNode | null;

  while (parent && shouldBubbleTransparentWrapper(parent, current, includeAssignmentRhs)) {
    current = parent;
    parent = current.parent() as AnyNode | null;
  }

  return current;
}

function unwrapTransparentWrappers(node: AnyNode): AnyNode {
  return unwrapTransparentWrappersInternal(node, true);
}

export function unwrapParenthesizedExpression(node: AnyNode): AnyNode {
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

function findEffectiveParentContext(node: AnyNode): EffectiveParentContext {
  const effectiveNode = unwrapTransparentWrappers(node);
  return {
    node: effectiveNode,
    parent: (effectiveNode.parent() as AnyNode | null) ?? null,
  };
}

export function isUsedAsConstructor(node: AnyNode): boolean {
  const { node: effectiveNode, parent } = findEffectiveParentContext(node);
  return (
    parent?.kind() === "new_expression" &&
    getFieldNode(parent, "constructor")?.id() === effectiveNode.id()
  );
}

export function isUsedInReflectiveAccess(node: AnyNode, keys: string[] = []): boolean {
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
    const key = getStaticStringKey(getFieldNode(parent, "index"));
    return key ? keySet.has(key) : false;
  }

  if (
    parent.kind() === "binary_expression" &&
    getFieldNode(parent, "right")?.id() === effectiveNode.id() &&
    getFieldText(parent, "operator") === "in"
  ) {
    const key = getStaticStringKey(getFieldNode(parent, "left"));
    return key ? keySet.has(key) : false;
  }

  return false;
}

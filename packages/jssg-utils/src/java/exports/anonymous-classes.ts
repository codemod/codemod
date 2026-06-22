import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";

export type JavaNode = SgNode<Java>;

export function getAnonymousClassMethod(
  classCreationNode: JavaNode,
  methodName: string,
): JavaNode | null {
  if (classCreationNode.kind() !== "object_creation_expression") {
    return null;
  }

  const classBody = classCreationNode.find({ rule: { kind: "class_body" } });
  if (!classBody) {
    return null;
  }

  for (const method of classBody.findAll({
    rule: { kind: "method_declaration" },
  })) {
    const currentMethodName = method
      .children()
      .find((child) => child.kind() === "identifier")
      ?.text();
    if (currentMethodName === methodName) {
      return method;
    }
  }

  return null;
}

export function getAnonymousClassMethods(
  classCreationNode: JavaNode,
  methodNames: string[],
): Map<string, JavaNode> | null {
  const methods = new Map<string, JavaNode>();
  for (const methodName of methodNames) {
    const method = getAnonymousClassMethod(classCreationNode, methodName);
    if (!method) {
      return null;
    }
    methods.set(methodName, method);
  }

  return methods;
}

export function getSingleParameterName(method: JavaNode): string | null {
  const params = method.find({ rule: { kind: "formal_parameters" } });
  const parameters =
    params
      ?.children()
      .filter(
        (child) => child.kind() === "formal_parameter" || child.kind() === "spread_parameter",
      ) ?? [];
  if (parameters.length !== 1) {
    return null;
  }

  const parameter = parameters[0];
  if (!parameter) {
    return null;
  }

  const nameNode = parameter
    .children()
    .filter((child) => child.isNamed())
    .reverse()
    .find((child) => child.kind() === "identifier");
  return nameNode?.text() ?? null;
}

export function getMethodBodyContent(
  method: JavaNode,
  rename?: { from: string; to: string },
): string | null {
  const block = method.find({ rule: { kind: "block" } });
  if (!block) {
    return null;
  }

  const text = rename ? renameIdentifiersInNode(block, rename.from, rename.to) : block.text();
  return getBlockBodyText(text);
}

export function renameIdentifiersInNode(node: JavaNode, from: string, to: string): string {
  const edits = node
    .findAll({ rule: { kind: "identifier" } })
    .filter((identifier) => identifier.text() === from)
    .map((identifier) => identifier.replace(to));

  return edits.length > 0 ? node.commitEdits(edits) : node.text();
}

function getBlockBodyText(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) {
    return null;
  }

  return trimmed.slice(1, -1).trim();
}

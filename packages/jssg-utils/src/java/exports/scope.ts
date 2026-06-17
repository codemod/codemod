import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";

export type JavaNode = SgNode<Java>;

export type DeclarationInfo = {
  kind: "parameter" | "local" | "field";
  name: string;
  typeText: string | null;
  node: JavaNode;
};

export function findVisibleDeclarationBeforeUsage(options: {
  usageNode: JavaNode;
  name: string;
}): DeclarationInfo | null {
  const { usageNode, name } = options;

  for (const ancestor of usageNode.ancestors()) {
    if (
      ancestor.kind() === "method_declaration" ||
      ancestor.kind() === "constructor_declaration" ||
      ancestor.kind() === "lambda_expression"
    ) {
      const parameter = findDirectParameter(ancestor, name);
      if (parameter) {
        return parameter;
      }
    }

    if (ancestor.kind() !== "block") {
      continue;
    }

    const local = findPriorLocalDeclaration(ancestor, usageNode, name);
    if (local) {
      return local;
    }
  }

  const enclosingClass = findEnclosingNode(usageNode, "class_declaration");
  return enclosingClass ? findFieldDeclaration(enclosingClass, name) : null;
}

export function findEnclosingNode(node: JavaNode, kind: string): JavaNode | null {
  return node.ancestors().find((ancestor) => ancestor.kind() === kind) ?? null;
}

export function findDirectChild(parent: JavaNode, descendant: JavaNode): JavaNode | null {
  let current: JavaNode | null = descendant;
  while (current?.parent() && current.parent()?.id() !== parent.id()) {
    current = current.parent();
  }

  return current?.parent()?.id() === parent.id() ? current : null;
}

export function findTypeNode(node: JavaNode): JavaNode | null {
  return (
    node
      .children()
      .filter((child) => child.isNamed())
      .find((child) =>
        [
          "generic_type",
          "type_identifier",
          "scoped_type_identifier",
          "integral_type",
          "floating_point_type",
          "boolean_type",
          "void_type",
        ].includes(child.kind()),
      ) ?? null
  );
}

function findDirectParameter(scope: JavaNode, name: string): DeclarationInfo | null {
  const parameters = scope.find({ rule: { kind: "formal_parameters" } });
  if (!parameters) {
    return null;
  }

  for (const parameter of parameters.children()) {
    if (parameter.kind() !== "formal_parameter" && parameter.kind() !== "spread_parameter") {
      continue;
    }

    if (parameter.field("name")?.text() !== name) {
      continue;
    }

    return {
      kind: "parameter",
      name,
      typeText: findTypeNode(parameter)?.text() ?? null,
      node: parameter,
    };
  }

  return null;
}

function findPriorLocalDeclaration(
  block: JavaNode,
  usageNode: JavaNode,
  name: string,
): DeclarationInfo | null {
  const containingChild = findDirectChild(block, usageNode);
  if (!containingChild) {
    return null;
  }

  for (const child of block.children()) {
    if (child.id() === containingChild.id()) {
      break;
    }

    if (child.kind() !== "local_variable_declaration") {
      continue;
    }

    const typeText = findTypeNode(child)?.text() ?? null;
    for (const declarator of child.fieldChildren("declarator")) {
      if (declarator.field("name")?.text() === name) {
        return {
          kind: "local",
          name,
          typeText,
          node: declarator,
        };
      }
    }
  }

  return null;
}

function findFieldDeclaration(classNode: JavaNode, name: string): DeclarationInfo | null {
  for (const field of classNode.findAll({ rule: { kind: "field_declaration" } })) {
    const typeText = findTypeNode(field)?.text() ?? null;
    for (const declarator of field.fieldChildren("declarator")) {
      if (declarator.field("name")?.text() === name) {
        return {
          kind: "field",
          name,
          typeText,
          node: declarator,
        };
      }
    }
  }

  return null;
}

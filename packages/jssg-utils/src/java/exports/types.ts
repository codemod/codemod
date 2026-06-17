import type Java from "@codemod.com/jssg-types/langs/java";
import type { Edit, SgNode } from "@codemod.com/jssg-types/main";
import {
  collectImports,
  hasConflictingSimpleImport,
  isTypeImported,
  simpleName as getSimpleName,
} from "./imports";

export type JavaNode = SgNode<Java>;

export function isTypeShadowed(
  rootNode: JavaNode,
  options: { simpleName: string; expectedFullyQualifiedName: string },
): boolean {
  const imports = collectImports(rootNode);
  if (hasConflictingSimpleImport(imports, options)) {
    return true;
  }

  return rootNode.findAll({ rule: { kind: "class_declaration" } }).some((classNode) => {
    const identifier = classNode.children().find((child) => child.kind() === "identifier");
    return identifier?.text() === options.simpleName;
  });
}

export function isKnownType(
  rootNode: JavaNode,
  typeText: string,
  fullyQualifiedName: string,
): boolean {
  const baseName = baseTypeName(typeText);
  if (baseName === fullyQualifiedName) {
    return true;
  }

  const simpleName = getSimpleName(fullyQualifiedName);
  if (baseName !== simpleName) {
    return false;
  }

  const imports = collectImports(rootNode);
  return (
    isTypeImported(imports, { simpleName, fullyQualifiedName }) &&
    !hasConflictingSimpleImport(imports, {
      simpleName,
      expectedFullyQualifiedName: fullyQualifiedName,
    }) &&
    !isTypeShadowed(rootNode, { simpleName, expectedFullyQualifiedName: fullyQualifiedName })
  );
}

export function replaceTypeIdentifierSafely(node: JavaNode, replacement: string): Edit | null {
  if (isInsideImport(node)) {
    return null;
  }

  const parent = node.parent();
  if (
    node.kind() === "type_identifier" &&
    parent?.kind() === "scoped_type_identifier" &&
    parent.text() !== node.text()
  ) {
    return null;
  }

  return node.replace(replaceBaseTypeName(node.text(), replacement));
}

export function baseTypeName(typeText: string): string {
  const genericStart = typeText.indexOf("<");
  return (genericStart === -1 ? typeText : typeText.slice(0, genericStart)).trim();
}

export function replaceBaseTypeName(typeText: string, replacement: string): string {
  const genericStart = typeText.indexOf("<");
  const suffix = genericStart === -1 ? "" : typeText.slice(genericStart);
  return `${replacement}${suffix}`;
}

function isInsideImport(node: JavaNode): boolean {
  return node.ancestors().some((ancestor) => ancestor.kind() === "import_declaration");
}

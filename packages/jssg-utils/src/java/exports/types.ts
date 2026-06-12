import type Java from "@codemod.com/jssg-types/langs/java";
import type { Edit, SgNode } from "@codemod.com/jssg-types/main";
import {
  collectJavaImports,
  hasConflictingJavaSimpleImport,
  isJavaTypeImported,
  simpleJavaName,
} from "./imports";

export type JavaNode = SgNode<Java>;

export function isJavaTypeShadowed(
  rootNode: JavaNode,
  options: { simpleName: string; expectedFullyQualifiedName: string },
): boolean {
  const imports = collectJavaImports(rootNode);
  if (hasConflictingJavaSimpleImport(imports, options)) {
    return true;
  }

  return rootNode.findAll({ rule: { kind: "class_declaration" } }).some((classNode) => {
    const identifier = classNode.children().find((child) => child.kind() === "identifier");
    return identifier?.text() === options.simpleName;
  });
}

export function isKnownJavaType(
  rootNode: JavaNode,
  typeText: string,
  fullyQualifiedName: string,
): boolean {
  const baseName = baseJavaTypeName(typeText);
  if (baseName === fullyQualifiedName) {
    return true;
  }

  const simpleName = simpleJavaName(fullyQualifiedName);
  if (baseName !== simpleName) {
    return false;
  }

  const imports = collectJavaImports(rootNode);
  return (
    isJavaTypeImported(imports, { simpleName, fullyQualifiedName }) &&
    !hasConflictingJavaSimpleImport(imports, {
      simpleName,
      expectedFullyQualifiedName: fullyQualifiedName,
    }) &&
    !isJavaTypeShadowed(rootNode, { simpleName, expectedFullyQualifiedName: fullyQualifiedName })
  );
}

export function replaceJavaTypeIdentifierSafely(node: JavaNode, replacement: string): Edit | null {
  if (isInsideJavaImport(node)) {
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

  return node.replace(replaceJavaBaseTypeName(node.text(), replacement));
}

export function baseJavaTypeName(typeText: string): string {
  const genericStart = typeText.indexOf("<");
  return (genericStart === -1 ? typeText : typeText.slice(0, genericStart)).trim();
}

export function replaceJavaBaseTypeName(typeText: string, replacement: string): string {
  const genericStart = typeText.indexOf("<");
  const suffix = genericStart === -1 ? "" : typeText.slice(genericStart);
  return `${replacement}${suffix}`;
}

function isInsideJavaImport(node: JavaNode): boolean {
  return node.ancestors().some((ancestor) => ancestor.kind() === "import_declaration");
}

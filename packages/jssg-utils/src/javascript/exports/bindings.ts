import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { Kinds, SgNode } from "@codemod.com/jssg-types/main";

type Language = JS | TS | TSX;

function findAncestorOfKind<L extends Language, T extends SgNode<L>>(node: T, kinds: Kinds<L>[]) {
  return node.ancestors().find((ancestor) => kinds.includes(ancestor.kind())) ?? null;
}

function isNodeWithin(node: SgNode<Language>, container: SgNode<Language> | null) {
  if (!container) {
    return false;
  }

  return (
    node.id() === container.id() ||
    node.ancestors().some((ancestor) => ancestor.id() === container.id())
  );
}

function isDeclarationSite(node: SgNode<Language>) {
  if (
    findAncestorOfKind(node, ["import_specifier", "import_clause", "import_statement"]) !== null
  ) {
    return true;
  }

  const declarator = findAncestorOfKind(node, ["variable_declarator"]);
  if (
    declarator &&
    isNodeWithin(node, (declarator.field("name") as SgNode<Language> | null) ?? null)
  ) {
    return true;
  }

  const functionDeclaration = findAncestorOfKind(node, ["function_declaration"]);
  if (
    functionDeclaration &&
    isNodeWithin(node, (functionDeclaration.field("name") as SgNode<Language> | null) ?? null)
  ) {
    return true;
  }

  const classDeclaration = findAncestorOfKind(node, ["class_declaration"]);
  if (
    classDeclaration &&
    isNodeWithin(node, (classDeclaration.field("name") as SgNode<Language> | null) ?? null)
  ) {
    return true;
  }

  return false;
}

function isTypeOnlyImportDefinition(node: SgNode<Language>) {
  const importSpecifier = findAncestorOfKind(node, ["import_specifier"]);
  if (importSpecifier?.text().trimStart().startsWith("type ")) {
    return true;
  }

  const importStatement = findAncestorOfKind(node, ["import_statement"]);
  return importStatement?.text().startsWith("import type ") ?? false;
}

function isDirectRequireCall(node: SgNode<Language> | null) {
  return !!node?.is("call_expression") && node.field("function")?.text() === "require";
}

function isDirectRequireBindingDefinition(node: SgNode<Language>) {
  const declarator = findAncestorOfKind(node, ["variable_declarator"]);
  const valueField = (declarator?.field("value") as SgNode<Language> | null) ?? null;
  return isDirectRequireCall(valueField);
}

/**
 * Returns `true` when `node` resolves to a runtime import binding.
 *
 * This accepts both ESM imports and CommonJS `require(...)` bindings, including
 * destructured aliases such as `const { Grid: LocalGrid } = require(...)`.
 *
 * This returns `false` for declaration sites and local shadows, so it is safe to
 * use as a conservative gate before rewriting an identifier or JSX tag as an
 * imported runtime symbol.
 *
 * @param node The identifier or JSX identifier node to check.
 * @returns `true` when the node resolves to a runtime import binding; otherwise `false`.
 */
export function isRuntimeImportBinding<T extends SgNode<Language>>(node: T) {
  if (isDeclarationSite(node)) return false;

  const def = node?.definition({ resolveExternal: false });
  if (!def?.node) return false;

  if (isTypeOnlyImportDefinition(def.node)) return false;

  const isNamedImports = def?.node.parent()?.is("named_imports");
  const isImportSpecifier = def?.node.parent()?.is("import_specifier");
  const isImportStatement = def?.node.parent()?.is("import_statement");
  const isRequireBinding = isDirectRequireBindingDefinition(def.node);

  return isImportSpecifier || isNamedImports || isImportStatement || isRequireBinding;
}

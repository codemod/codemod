import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { SgNode } from "@codemod.com/jssg-types/main";

type Language = JS | TS | TSX;

function findAncestorOfKind<T extends Language>(node: SgNode<T>, kinds: string[]): any | null {
  return node.ancestors().find((ancestor: any) => kinds.includes(String(ancestor.kind()))) ?? null;
}

function isBoundToImport<T extends Language>(node: SgNode<T>): boolean {
  const def = node.definition({ resolveExternal: false });

  const isNamedImports = def?.node.parent()?.is("named_imports");
  const isImportSpecifier = def?.node.parent()?.is("import_specifier");
  const isImportStatement = def?.node.parent()?.is("import_statement");

  return isImportSpecifier || isNamedImports || isImportStatement || false;
}

export function findShadowingBinding<T extends Language>(node: SgNode<T>): SgNode<T> | null {
  if (isBoundToImport(node)) return null;
  const def = node.definition({ resolveExternal: false });
  return def?.node ?? null;
}

export function isRuntimeImportBinding<T extends Language>(node: SgNode<T>) {
  const isImportDeclarationSite =
    findAncestorOfKind(node, ["import_specifier", "import_clause", "import_statement"]) !== null;

  if (isImportDeclarationSite) return false;

  const def = node?.definition({ resolveExternal: false });

  const isTypeImport = def?.node.parent()?.parent()?.parent()?.text().includes("import type");

  if (isTypeImport) return false;

  const rule = {
    pattern: "require($ARG)",
    kind: "call_expression",
  };

  const isRequire = !!(def?.node.parent() as any | null)?.find({
    rule,
  } as any);

  const isPairPatternInRequire = !!(
    def?.node.parent()?.parent()?.parent()?.parent() as any | null
  )?.find({
    rule,
  } as any);

  return isBoundToImport(node) || isRequire || isPairPatternInRequire;
}

import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { Kinds, SgNode } from "@codemod.com/jssg-types/main";

type Language = JS | TS | TSX;

function findAncestorOfKind<L extends Language, T extends SgNode<L>>(node: T, kinds: Kinds<L>[]) {
  return node.ancestors().find((ancestor) => kinds.includes(ancestor.kind())) ?? null;
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
  const isImportDeclarationSite =
    findAncestorOfKind(node, ["import_specifier", "import_clause", "import_statement"]) !== null;

  if (isImportDeclarationSite) return false;

  const def = node?.definition({ resolveExternal: false });

  const rule = {
    pattern: "require($ARG)",
    kind: "call_expression",
  } as const;

  const isRequire = !!def?.node.parent()?.find({
    rule,
  });

  const isNamedImports = def?.node.parent()?.is("named_imports");
  const isImportSpecifier = def?.node.parent()?.is("import_specifier");
  const isImportStatement = def?.node.parent()?.is("import_statement");
  const declarator = def?.node ? findAncestorOfKind(def.node, ["variable_declarator"]) : null;
  const valueField = declarator?.field("value");
  const isRequireBinding = !!valueField?.find({
    rule,
  });

  return isImportSpecifier || isNamedImports || isImportStatement || isRequire || isRequireBinding;
}

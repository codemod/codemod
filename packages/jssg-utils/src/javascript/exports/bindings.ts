import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { SgNode } from "@codemod.com/jssg-types/main";
import { getAllImports } from "./imports.ts";

type Language = JS | TS | TSX;

type ImportBindingQuery =
  | {
      type: "default";
      from: string;
    }
  | {
      type: "named";
      name: string;
      from: string;
    };

type ImportBinding<T extends Language> = {
  alias: string;
  isNamespace: boolean;
  moduleType: "esm" | "cjs";
  node: SgNode<T, "identifier">;
  isTypeOnly: boolean;
};

const SCOPE_KINDS = new Set<string>([
  "program",
  "statement_block",
  "for_statement",
  "for_in_statement",
  "for_of_statement",
  "function_declaration",
  "function_expression",
  "arrow_function",
  "generator_function_declaration",
  "generator_function",
  "method_definition",
]);

const HOIST_SCOPE_KINDS = new Set<string>([
  "program",
  "function_declaration",
  "function_expression",
  "arrow_function",
  "generator_function_declaration",
  "generator_function",
  "method_definition",
]);

const PARAMETER_BINDING_PARENT_KINDS = new Set<string>([
  "required_parameter",
  "optional_parameter",
  "rest_pattern",
  "shorthand_property_identifier_pattern",
  "pair_pattern",
  "array_pattern",
  "object_pattern",
]);

function findAncestorOfKind(node: any, kind: string): any | null {
  return node.ancestors().find((ancestor: any) => String(ancestor.kind()) === kind) ?? null;
}

function findNearestAncestorOfKinds(node: any, kinds: Set<string>): any | null {
  return node.ancestors().find((ancestor: any) => kinds.has(String(ancestor.kind()))) ?? null;
}

function findNearestScope(node: any): any | null {
  if (SCOPE_KINDS.has(String(node.kind()))) {
    return node;
  }

  return node.ancestors().find((ancestor: any) => SCOPE_KINDS.has(String(ancestor.kind()))) ?? null;
}

function getOuterScope(scope: any): any | null {
  return (
    scope.ancestors().find((ancestor: any) => SCOPE_KINDS.has(String(ancestor.kind()))) ?? null
  );
}

function sameNode(left: any | null, right: any | null) {
  return left !== null && right !== null && left.id() === right.id();
}

function isWithinSubtree(node: any, subtree: any | null) {
  return (
    subtree !== null &&
    (node.id() === subtree.id() ||
      node.ancestors().some((ancestor: any) => ancestor.id() === subtree.id()))
  );
}

function belongsToScope(node: any, scope: any) {
  const nearestScope = findNearestScope(node);
  return nearestScope !== null && nearestScope.id() === scope.id();
}

function isVariableBindingIdentifier(node: any) {
  const declarator = findAncestorOfKind(node, "variable_declarator");
  if (!declarator) {
    return false;
  }

  const nameField = declarator.field("name") as any | null;
  return isWithinSubtree(node, nameField);
}

function isFunctionLikeNameIdentifier(node: any) {
  const parent = node.parent();
  if (!parent) {
    return false;
  }

  if (
    String(parent.kind()) !== "function_declaration" &&
    String(parent.kind()) !== "class_declaration"
  ) {
    return false;
  }

  const nameField = parent.field("name") as any | null;
  return nameField !== null && nameField.id() === node.id();
}

function isImportBindingIdentifier(node: any) {
  return (
    String(node.kind()) === "import_specifier" ||
    String(node.kind()) === "namespace_import" ||
    String(node.kind()) === "import_clause" ||
    findAncestorOfKind(node, "import_specifier") !== null ||
    findAncestorOfKind(node, "namespace_import") !== null ||
    findAncestorOfKind(node, "import_clause") !== null
  );
}

function isParameterBindingIdentifier(node: any, scope: any) {
  const params = findAncestorOfKind(node, "formal_parameters");
  if (!params) {
    return false;
  }

  if (!params || !belongsToScope(params, scope) || !sameNode(findNearestScope(node), scope)) {
    return false;
  }

  const parentKind = String(node.parent()?.kind());
  if (PARAMETER_BINDING_PARENT_KINDS.has(parentKind)) {
    return true;
  }

  const assignmentPattern = findAncestorOfKind(node, "assignment_pattern");
  if (assignmentPattern) {
    const leftField =
      (assignmentPattern.field("left") as any | null) ??
      (assignmentPattern.field("pattern") as any | null) ??
      assignmentPattern.child(0);
    return isWithinSubtree(node, leftField);
  }

  const parameter = findNearestAncestorOfKinds(
    node,
    new Set(["required_parameter", "optional_parameter", "rest_pattern"]),
  );
  if (!parameter) {
    return false;
  }

  const bindingField =
    (parameter.field("pattern") as any | null) ??
    (parameter.field("name") as any | null) ??
    parameter.child(0);

  return isWithinSubtree(node, bindingField);
}

function isCatchBindingIdentifier(node: any, scope: any) {
  const catchClause = findAncestorOfKind(node, "catch_clause");
  if (!catchClause || !sameNode(findNearestScope(node), scope)) {
    return false;
  }

  const parameterField =
    (catchClause.field("parameter") as any | null) ?? catchClause.child(1) ?? catchClause.child(0);

  return isWithinSubtree(node, parameterField);
}

function getDeclarationScope(node: any) {
  const declarator = findAncestorOfKind(node, "variable_declarator");
  if (declarator) {
    const declaration =
      findAncestorOfKind(declarator, "lexical_declaration") ??
      findAncestorOfKind(declarator, "variable_declaration");

    if (declaration && String(declaration.kind()) === "variable_declaration") {
      return findNearestAncestorOfKinds(declaration, HOIST_SCOPE_KINDS);
    }
  }

  if (isFunctionLikeNameIdentifier(node)) {
    const declaration = node.parent();
    return declaration ? getOuterScope(declaration) : null;
  }

  if (findAncestorOfKind(node, "catch_clause")) {
    return findNearestScope(node);
  }

  return findNearestScope(node);
}

function findLocalDefinition(node: any, identifierName: string) {
  if (typeof node.definition !== "function") {
    return null;
  }

  const definition = node.definition({ resolveExternal: false });
  if (!definition || definition.kind !== "local") {
    return null;
  }

  if (sameNode(definition.node, node) || definition.node.text() !== identifierName) {
    return null;
  }

  if (isImportBindingIdentifier(definition.node)) {
    return null;
  }

  return definition.node;
}

function findDeclarationsInScope(scope: any, identifierName: string): any[] {
  const matchingIdentifiers = scope
    .findAll({
      rule: {
        any: [{ kind: "identifier" }, { kind: "shorthand_property_identifier_pattern" }],
      },
    })
    .filter((candidate: any) => candidate.text() === identifierName) as any[];

  return matchingIdentifiers.filter((candidate) => {
    const declarationScope = getDeclarationScope(candidate);
    if (!sameNode(declarationScope, scope)) {
      return false;
    }

    return (
      isVariableBindingIdentifier(candidate) ||
      isFunctionLikeNameIdentifier(candidate) ||
      isParameterBindingIdentifier(candidate, scope) ||
      isCatchBindingIdentifier(candidate, scope)
    );
  });
}

function isTypeOnlyImportBinding<T extends Language>(node: SgNode<T>) {
  const importSpecifier = findAncestorOfKind(node, "import_specifier");
  if (importSpecifier && /^\s*type\b/.test(importSpecifier.text())) {
    return true;
  }

  const importStatement = findAncestorOfKind(node, "import_statement");
  if (importStatement && /^\s*import\s+type\b/.test(importStatement.text())) {
    return true;
  }

  return false;
}

function getAllTopLevelImportBindings<T extends Language>(
  program: SgNode<T, "program">,
  query: ImportBindingQuery,
): ImportBinding<T>[] {
  return getAllImports(program, query).map(
    (binding: {
      alias: string;
      isNamespace: boolean;
      moduleType: "esm" | "cjs";
      node: SgNode<T, "identifier">;
    }) => ({
      ...binding,
      isTypeOnly: isTypeOnlyImportBinding(binding.node),
    }),
  );
}

export function findShadowingBinding<T extends Language>(
  node: SgNode<T>,
  identifierName: string,
): SgNode<T> | null {
  const localDefinition = findLocalDefinition(node, identifierName);
  if (localDefinition) {
    return localDefinition as SgNode<T>;
  }

  let scope = findNearestScope(node);

  while (scope) {
    const match = findDeclarationsInScope(scope, identifierName).find(
      (candidate) => !sameNode(candidate, node),
    );

    if (match) {
      return match;
    }

    scope = getOuterScope(scope);
  }

  return null;
}

function isNodeBoundToIdentifier<T extends Language>(node: SgNode<T>, identifierName: string) {
  return (
    String(node.kind()) === "identifier" &&
    node.text() === identifierName &&
    !findShadowingBinding(node, identifierName)
  );
}

export function isRuntimeImportBinding<T extends Language>(
  node: SgNode<T>,
  query: ImportBindingQuery,
) {
  const program = node.getRoot().root() as SgNode<T, "program">;
  const runtimeBindings = getAllTopLevelImportBindings(program, query).filter(
    (binding) => !binding.isTypeOnly,
  );

  if (runtimeBindings.length === 0 || String(node.kind()) !== "identifier") {
    return false;
  }

  return runtimeBindings.some(
    (binding) => binding.alias === node.text() && isNodeBoundToIdentifier(node, binding.alias),
  );
}

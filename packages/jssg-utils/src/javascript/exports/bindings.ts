import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type { Edit, SgNode } from "@codemod.com/jssg-types/main";

type Language = JS | TS | TSX;
type Node = SgNode<TS>;

export interface ImportBinding {
  /** Exported name. `"default"` for a default import, `"*"` for `import * as`. */
  importedName: string;
  /** Name used at the use site. */
  localName: string;
  /** Module specifier of the import in the file that contains the use. */
  source: string;
}

function asNode<T extends Language>(node: SgNode<T>): Node {
  return node as unknown as Node;
}

function safeDefinition(node: Node, resolveExternal = true) {
  try {
    return node.definition({ resolveExternal });
  } catch {
    return null;
  }
}

function safeReferences(node: Node) {
  try {
    return node.references();
  } catch {
    return [];
  }
}

function asSpecifier(node: Node): Node | null {
  if (node.kind() === "import_specifier") return node;
  const parent = node.parent();
  if (parent?.kind() === "import_specifier") return parent;
  return null;
}

function moduleSource(node: Node): string | null {
  const statement =
    node.kind() === "import_statement"
      ? node
      : node.ancestors().find((ancestor) => ancestor.kind() === "import_statement");
  const sourceNode = statement?.field("source");
  if (!sourceNode) return null;
  const fragments = sourceNode.findAll({ rule: { kind: "string_fragment" } });
  if (fragments.length === 0) return null;
  return fragments.map((fragment) => fragment.text()).join("");
}

function readNamedSpecifier(specifier: Node): ImportBinding | null {
  const nameNode = specifier.field("name");
  if (!nameNode) return null;
  const source = moduleSource(specifier);
  if (source === null) return null;
  const alias = specifier.field("alias");
  return {
    importedName: nameNode.text(),
    localName: alias ? alias.text() : nameNode.text(),
    source,
  };
}

function bindingInStatement(statement: Node, localName: string): ImportBinding | null {
  const source = moduleSource(statement);
  if (source === null) return null;

  for (const specifier of statement.findAll({ rule: { kind: "import_specifier" } })) {
    const nameNode = specifier.field("name");
    const alias = specifier.field("alias");
    const local = alias ?? nameNode;
    if (!nameNode || local?.text() !== localName) continue;
    return { importedName: nameNode.text(), localName, source };
  }

  const clause = statement.find({ rule: { kind: "import_clause" } });
  if (!clause) return null;

  for (const child of clause.children()) {
    if (child.kind() === "identifier" && child.text() === localName) {
      return { importedName: "default", localName, source };
    }
    if (child.kind() === "namespace_import") {
      const identifier = child.children().find((inner) => inner.kind() === "identifier");
      if (identifier?.text() === localName) {
        return { importedName: "*", localName, source };
      }
    }
  }

  return null;
}

function bindingFromDefinedNode(defined: Node, localNameHint: string): ImportBinding | null {
  const specifier = asSpecifier(defined);
  if (specifier) return readNamedSpecifier(specifier);

  if (defined.kind() === "identifier") {
    const parent = defined.parent();
    const source = moduleSource(defined);
    if (source !== null && parent?.kind() === "namespace_import") {
      return { importedName: "*", localName: defined.text(), source };
    }
    if (source !== null && parent?.kind() === "import_clause") {
      return { importedName: "default", localName: defined.text(), source };
    }
  }

  const statement =
    defined.kind() === "import_statement"
      ? defined
      : defined.ancestors().find((ancestor) => ancestor.kind() === "import_statement");
  if (!statement) return null;
  return bindingInStatement(statement, localNameHint);
}

/**
 * Import that defines this use, or null when the name is local.
 *
 * Stops at the import in the file that contains the use. A re-export is
 * reported as that file's specifier (`"./browser"`), not the package the
 * other file imported from.
 *
 * `ns.member` after `import * as ns` is not resolved. Call this on `ns`.
 * Named imports compare `importedName` with the exported name, so
 * `import { setup as create }` reports `setup`, not `create`.
 *
 * Returns null when semantic analysis is unavailable. `definition()` can
 * throw in that case; this function does not.
 */
export function importBindingOf<T extends Language>(node: SgNode<T>): ImportBinding | null {
  const current = asNode(node);
  const definition = safeDefinition(current, false);
  if (!definition) return null;
  return bindingFromDefinedNode(definition.node, current.text());
}

function resolveDefinitionNode(node: Node): Node | null {
  const definition = safeDefinition(node);
  if (!definition) return null;

  const specifier = asSpecifier(definition.node);
  if (!specifier) return definition.node;

  const localNode = specifier.field("alias") ?? specifier.field("name");
  if (!localNode || localNode.id() === node.id()) {
    return definition.kind === "external" ? definition.node : specifier;
  }

  const followed = safeDefinition(localNode);
  if (followed?.kind === "external") return followed.node;
  return specifier;
}

/**
 * The node that defines this use, following one import into another project file.
 *
 * A local declaration returns that node. An import the workspace can resolve
 * (`kind: "external"`, which means another project file) returns that file's
 * definition. An unresolved package import returns the import specifier.
 *
 * Does not follow a second hop. `const a = setup(); const b = a` resolves `b`
 * to `b`, not to `setup()`.
 */
export function resolveDefinition<T extends Language>(node: SgNode<T>): SgNode<T> | null {
  const resolved = resolveDefinitionNode(asNode(node));
  return resolved as unknown as SgNode<T> | null;
}

function declaratorOf(node: Node): Node | null {
  const resolved = resolveDefinitionNode(node);
  if (!resolved) return null;

  if (resolved.kind() === "identifier" && resolved.parent()?.kind() === "variable_declarator") {
    const parent = resolved.parent();
    return parent ?? null;
  }
  if (resolved.kind() === "variable_declarator") return resolved;
  return null;
}

function calleeOf(expression: Node): Node | null {
  if (expression.kind() === "call_expression") return expression.field("function");
  if (expression.kind() === "new_expression") return expression.field("constructor");
  return null;
}

/**
 * True when `node` is a variable initialized by a direct call, or `new`, of
 * `importedName` from one of `sources`.
 *
 * One hop only, including through an import of that variable from another
 * project file. These stay false:
 * - `const { listen } = setup()`
 * - `lib.setup()`
 * - `const a = setup(); const b = a`
 * - a shadowed local with the same spelling
 */
export function resolvesToFactory<T extends Language>(
  node: SgNode<T>,
  importedName: string,
  sources: readonly string[],
): boolean {
  const declarator = declaratorOf(asNode(node));
  if (!declarator) return false;

  const value = declarator.field("value");
  if (!value) return false;
  const callee = calleeOf(value);
  if (!callee || callee.kind() !== "identifier") return false;

  const binding = importBindingOf(callee);
  return (
    binding !== null && binding.importedName === importedName && sources.includes(binding.source)
  );
}

function isInsideImport(node: Node): boolean {
  if (node.kind() === "import_statement") return true;
  return node.ancestors().some((ancestor) => ancestor.kind() === "import_statement");
}

/**
 * Edits that rename references of `binding` in `currentFile`.
 *
 * Shadowed bindings are left alone, because `references()` only returns this
 * binding. Other files are left alone: workspace `references()` includes them,
 * and those edits do not belong on the file currently being committed. Pass
 * `root.filename()` as `currentFile`.
 *
 * A shorthand property is replaced as an identifier, so `{ oldName }` becomes
 * `{ newName }` and the key changes. The import clause itself is not renamed;
 * use `updateImport` for that.
 *
 * Returns `[]` when semantic analysis is unavailable.
 */
export function renameReferences<T extends Language>(
  binding: SgNode<T>,
  toName: string,
  currentFile: string,
): Edit[] {
  const edits: Edit[] = [];

  for (const file of safeReferences(asNode(binding))) {
    if (file.root.filename() !== currentFile) continue;

    for (const reference of file.nodes) {
      if (reference.id() === binding.id()) continue;
      if (isInsideImport(reference)) continue;
      edits.push(reference.replace(toName));
    }
  }

  return edits;
}

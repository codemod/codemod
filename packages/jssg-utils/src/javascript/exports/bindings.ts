// The namespace on purpose: a CLI without `symbols` still loads this module,
// and these helpers answer null there instead of failing at import.
import * as sg from "codemod:ast-grep";
import type JS from "codemod/langs/javascript";
import type TS from "codemod/langs/typescript";
import type TSX from "codemod/langs/tsx";
import type { Edit, SgNode } from "codemod/main";

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

interface Definition {
  node: Node;
  kind: string;
}

type SymbolsOf = (language: "typescript") => {
  definition: { get(node: Node): ReadonlyArray<Definition> | undefined };
  references: { get(node: Node): ReadonlyArray<{ node: Node }> | undefined };
};

/** `symbols(lang)` on a CLI that has it. Its lookups throw without semantic analysis. */
function lookups() {
  const symbols = (sg as unknown as { symbols?: SymbolsOf }).symbols;
  return symbols ? symbols("typescript") : null;
}

/** Where the name is declared, through imports. Null when the step has no semantic analysis. */
function safeDefinition(node: Node): Definition | null {
  try {
    const found = lookups();
    if (found) return found.definition.get(node)?.[0] ?? null;
    // CLIs before `symbols(lang)` answer on the node.
    const legacy = node as unknown as { definition?(o: object): Definition | null };
    return legacy.definition?.({ resolveExternal: true, follow: true }) ?? null;
  } catch {
    return null;
  }
}

function safeReferences(node: Node): Node[] {
  try {
    const found = lookups();
    if (found) return (found.references.get(node) ?? []).map((edge) => edge.node);
    const legacy = node as unknown as { references?(): { nodes: Node[] }[] };
    return (legacy.references?.() ?? []).flatMap((file) => file.nodes);
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

/** The name an import statement binds as `localName`: the alias, default or namespace identifier. */
function importedLocal(statement: Node, localName: string): Node | null {
  for (const specifier of statement.findAll({ rule: { kind: "import_specifier" } })) {
    const local = specifier.field("alias") ?? specifier.field("name");
    if (local?.text() === localName) return local;
  }
  const clause = statement.find({ rule: { kind: "import_clause" } });
  if (!clause) return null;
  for (const child of clause.children()) {
    if (child.kind() === "identifier" && child.text() === localName) return child;
    if (child.kind() === "namespace_import") {
      const identifier = child.children().find((inner) => inner.kind() === "identifier");
      if (identifier?.text() === localName) return identifier;
    }
  }
  return null;
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
 * A use counts when it means the same declaration as the import's own name,
 * so a shadowing local is not the import. Returns null when semantic analysis
 * is unavailable; this function does not throw.
 */
export function importBindingOf<T extends Language>(node: SgNode<T>): ImportBinding | null {
  const current = asNode(node);
  const name = current.text();
  for (const statement of current.getRoot().root().findAll({ rule: { kind: "import_statement" } })) {
    const local = importedLocal(statement, name);
    if (!local) continue;
    const binding = bindingInStatement(statement, name);
    if (!binding) continue;
    if (local.equals(current)) return binding;
    const used = safeDefinition(current);
    const imported = safeDefinition(local);
    if (used && imported && used.node.equals(imported.node)) return binding;
    return null;
  }
  return null;
}

function resolveDefinitionNode(node: Node): Node | null {
  const definition = safeDefinition(node);
  if (!definition) return null;
  // An import the workspace couldn't follow stays on its specifier.
  return asSpecifier(definition.node) ?? definition.node;
}

/**
 * The node that defines this use, following imports into other project files.
 *
 * A local declaration returns that node. An import the workspace can resolve
 * returns the declaration it leads to, through re-exports. An unresolved
 * package import returns the import specifier.
 *
 * Follows imports only, not values. `const a = setup(); const b = a` resolves `b`
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
 * Shadowed bindings are left alone, because `symbols(lang).references` only
 * returns this binding. Other files are left alone: in workspace scope the
 * references include them,
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

  for (const reference of safeReferences(asNode(binding))) {
    if (reference.getRoot().filename() !== currentFile) continue;
    if (reference.id() === binding.id()) continue;
    if (isInsideImport(reference)) continue;
    edits.push(reference.replace(toName));
  }

  return edits;
}

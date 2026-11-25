import type JS from "codemod/langs/javascript";
import type TSX from "codemod/langs/tsx";
import type TS from "codemod/langs/typescript";
import type { Edit, SgNode, SgRoot } from "codemod/main";

type Language = JS | TS | TSX;

/** One binding of an `import` statement. */
export type ImportedBinding<T extends Language = Language> = {
  /** The name used in this file. */
  local: string;
  /** What it asks the module for: `"default"` or the export name. */
  imported: string;
  /** `import type { … }`, or an inline `type` modifier. */
  typeOnly: boolean;
  /** The local name's identifier. */
  node: SgNode<T>;
};

/** Where a binding should be imported from instead. */
export type ImportTarget = {
  /** The module specifier to import it from. */
  from: string;
  /** The export to import: `"default"` or the export name. */
  name: string;
};

const asTS = <T extends Language>(node: SgNode<T>) => node as unknown as SgNode<TS>;
const asT = <T extends Language>(node: SgNode<TS>) => node as unknown as SgNode<T>;

/** The module specifier string of an import statement. */
function sourceOf<T extends Language>(statement: SgNode<T>): SgNode<TS> | null {
  const node = asTS(statement);
  return node.field("source") ?? node.children().find((c) => c.is("string")) ?? null;
}

/**
 * The default and named bindings of an ESM `import` statement. Empty for
 * namespace imports (`import * as ns`) and side-effect imports.
 */
export function importedBindings<T extends Language>(statement: SgNode<T>): ImportedBinding<T>[] {
  const node = asTS(statement);
  const clause = node.children().find((c) => c.is("import_clause"));
  if (!clause || clause.find({ rule: { kind: "namespace_import" } })) return [];
  const statementType = node.children().some((c) => c.is("type"));
  const bindings: ImportedBinding<T>[] = [];
  const defaultId = clause.children().find((c) => c.is("identifier"));
  if (defaultId) {
    bindings.push({
      local: defaultId.text(),
      imported: "default",
      typeOnly: statementType,
      node: asT<T>(defaultId),
    });
  }
  for (const spec of clause.findAll({ rule: { kind: "import_specifier" } })) {
    const name = spec.field("name") as SgNode<TS> | null;
    const alias = spec.field("alias") as SgNode<TS> | null;
    if (!name) continue;
    const local = alias ?? name;
    bindings.push({
      local: local.text(),
      imported: name.is("string") ? name.text().slice(1, -1) : name.text(),
      typeOnly: statementType || spec.children().some((c) => c.is("type")),
      node: asT<T>(local),
    });
  }
  return bindings;
}

/**
 * Split an `import` statement so that some of its bindings come from other
 * modules. `target` is asked once per binding and returns where to import it
 * from (module and export name), or null to keep it on the original module.
 *
 * - Bindings that stay come first. Bindings moved to the same module share
 *   one statement; a default export becomes a default import
 *   (`import Modal from "./modal"`), a second one for the same module
 *   `{ default as X }`.
 * - Local names, `import type` and inline `type` modifiers, the original
 *   quote style and the trailing semicolon are kept.
 * - Namespace and side-effect imports are left alone.
 *
 * @example
 * ```ts
 * // import { Button, Modal } from "@/ui";
 * retargetImport(statement, (b) => b.imported === "Modal" ? { from: "@/ui/modal", name: "default" } : null);
 * // import { Button } from "@/ui";
 * // import Modal from "@/ui/modal";
 * ```
 * @returns One edit replacing the statement, or null when nothing moves.
 */
export function retargetImport<T extends Language>(
  statement: SgNode<T>,
  target: (binding: ImportedBinding<T>) => ImportTarget | null,
): Edit | null {
  const source = sourceOf(statement);
  const bindings = importedBindings(statement);
  if (!source || bindings.length === 0) return null;
  const quote = source.text()[0] ?? '"';
  const original = source.text().slice(1, -1);
  const statementType = asTS(statement)
    .children()
    .some((c) => c.is("type"));
  const semicolon = statement.text().trimEnd().endsWith(";") ? ";" : "";

  type Group = { default?: string; named: string[] };
  const groups = new Map<string, Group>([[original, { named: [] }]]);
  let moved = false;
  for (const binding of bindings) {
    const to = target(binding) ?? { from: original, name: binding.imported };
    if (to.from !== original || to.name !== binding.imported) moved = true;
    let into = groups.get(to.from);
    if (!into) groups.set(to.from, (into = { named: [] }));
    const inlineType = binding.typeOnly && !statementType ? "type " : "";
    if (to.name === "default" && into.default === undefined && !inlineType) {
      into.default = binding.local;
    } else {
      into.named.push(
        inlineType + (to.name === binding.local ? to.name : `${to.name} as ${binding.local}`),
      );
    }
  }
  if (!moved) return null;

  const keyword = statementType ? "import type" : "import";
  const lines = [...groups]
    .filter(([, g]) => g.default !== undefined || g.named.length > 0)
    .map(([from, g]) => {
      const parts = [g.default, g.named.length ? `{ ${g.named.join(", ")} }` : undefined].filter(
        Boolean,
      );
      return `${keyword} ${parts.join(", ")} from ${quote}${from}${quote}${semicolon}`;
    });
  return statement.replace(lines.join("\n"));
}

/**
 * Import each binding of `statement` straight from the module that declares
 * it, instead of through a barrel. Needs semantic analysis in workspace scope.
 *
 * For each binding, `root.resolveExport()` follows the re-exports to the
 * declaration and `root.specifierFor()` writes a specifier for its file in
 * the style of the original (relative or alias, with or without `.js`).
 * A binding stays when it does not resolve, when it is declared in the
 * barrel itself, or when no specifier reaches its file.
 *
 * @example
 * ```ts
 * for (const statement of root.root().findAll({ rule: { kind: "import_statement" } })) {
 *   const edit = importFromDeclarations(root, statement);
 *   if (edit) edits.push(edit);
 * }
 * ```
 */
export function importFromDeclarations<T extends Language>(
  root: SgRoot<T>,
  statement: SgNode<T>,
): Edit | null {
  const source = sourceOf(statement);
  if (!source) return null;
  const from = source.text().slice(1, -1);
  const barrel = root.resolveModule(from);
  if (!barrel) return null;
  return retargetImport(statement, (binding) => {
    const declaration = root.resolveExport(barrel, binding.imported);
    if (!declaration?.exportName || declaration.root.filename() === barrel) return null;
    const specifier = root.specifierFor(declaration.root, { like: from });
    return specifier ? { from: specifier, name: declaration.exportName } : null;
  });
}

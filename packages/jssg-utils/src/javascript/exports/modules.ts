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
 * The bindings of an ESM `import` statement (default and named), or of an
 * `export { … } from` statement (`local` is the exported name, `imported`
 * the name asked of the module). Empty for namespace imports
 * (`import * as ns`), side-effect imports and `export * from`.
 */
export function importedBindings<T extends Language>(statement: SgNode<T>): ImportedBinding<T>[] {
  const node = asTS(statement);
  if (node.is("export_statement")) return reexportedBindings<T>(node);
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

function reexportedBindings<T extends Language>(node: SgNode<TS>): ImportedBinding<T>[] {
  const clause = node.children().find((c) => c.is("export_clause"));
  if (!clause || !sourceOf(node)) return [];
  const statementType = node.children().some((c) => c.is("type"));
  const bindings: ImportedBinding<T>[] = [];
  for (const spec of clause.findAll({ rule: { kind: "export_specifier" } })) {
    const name = spec.field("name") as SgNode<TS> | null;
    const alias = spec.field("alias") as SgNode<TS> | null;
    if (!name) continue;
    const exported = alias ?? name;
    const text = (n: SgNode<TS>) => (n.is("string") ? n.text().slice(1, -1) : n.text());
    bindings.push({
      local: text(exported),
      imported: text(name),
      typeOnly: statementType || spec.children().some((c) => c.is("type")),
      node: asT<T>(exported),
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
 *   quote style, the trailing semicolon and the file's line endings are kept. A moved group whose
 *   bindings are all type-only is written `import type { … }`.
 * - Namespace and side-effect imports are left alone.
 * - `export { … } from` statements work the same way: `export { Checkbox }
 *   from "./checkbox"` can become `export { Checkbox } from
 *   "./checkbox/Checkbox"`, and a default export is re-exported as
 *   `export { default as Modal } from "./modal"`. `export * from` is left alone.
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
  const isExport = asTS(statement).is("export_statement");
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
    if (!isExport && to.name === "default" && into.default === undefined && !inlineType) {
      into.default = binding.local;
    } else {
      into.named.push(
        inlineType + (to.name === binding.local ? to.name : `${to.name} as ${binding.local}`),
      );
    }
  }
  if (!moved) return null;

  const verb = isExport ? "export" : "import";
  const keyword = statementType ? `${verb} type` : verb;
  const lines = [...groups]
    .filter(([, g]) => g.default !== undefined || g.named.length > 0)
    .map(([from, g]) => {
      // A moved group of type-only bindings becomes `import type { A, B }`
      // (`export type { … }`).
      const allTypes =
        from !== original &&
        g.default === undefined &&
        g.named.every((named) => named.startsWith("type "));
      const named = allTypes ? g.named.map((n) => n.slice("type ".length)) : g.named;
      const parts = [g.default, named.length ? `{ ${named.join(", ")} }` : undefined].filter(
        Boolean,
      );
      const kw = allTypes ? `${verb} type` : keyword;
      return `${kw} ${parts.join(", ")} from ${quote}${from}${quote}${semicolon}`;
    });
  // Keep the file's line endings (CRLF files stay CRLF).
  const eol = statement.getRoot().root().text().includes("\r\n") ? "\r\n" : "\n";
  return statement.replace(lines.join(eol));
}

/**
 * Where each binding of `statement` (an `import` or `export { … } from`) is
 * declared: binding \`local\` name → \`{ from, name }\`, with a specifier written
 * in the style of the original (see \`importFromDeclarations\`). Bindings that
 * stay on the original module are left out. Needs workspace scope.
 *
 * @example
 * ```ts
 * const moves = declarationTargets(root, statement);
 * const modules = [...new Set([...moves.values()].map((t) => t.from))]; // where they went
 * ```
 */
export function declarationTargets<T extends Language>(
  root: SgRoot<T>,
  statement: SgNode<T>,
): Map<string, ImportTarget> {
  const targets = new Map<string, ImportTarget>();
  const source = sourceOf(statement);
  if (!source) return targets;
  const from = source.text().slice(1, -1);
  const barrel = root.resolveModule(from);
  // Installed packages are not debarrelled: their internals are not theirs to
  // import (vitest's `vi` lives in `dist/vendor/vi.<hash>`).
  if (!barrel || barrel.includes("/node_modules/")) return targets;
  for (const binding of importedBindings(statement)) {
    const declaration = root.resolveExport(barrel, binding.imported);
    if (!declaration?.exportName || declaration.root.filename() === barrel) continue;
    const specifier = root.specifierFor(declaration.root, { like: from });
    if (specifier) targets.set(binding.local, { from: specifier, name: declaration.exportName });
  }
  return targets;
}

/**
 * Import (or re-export) each binding of `statement` straight from the module
 * that declares it, instead of through a barrel. `statement` is an `import`
 * or an `export { … } from` statement. Needs semantic analysis in workspace
 * scope.
 *
 * For each binding, `root.resolveExport()` follows the re-exports to the
 * declaration and `root.specifierFor()` writes a specifier for its file in
 * the style of the original (relative or alias, with or without `.js`).
 * A binding stays when it does not resolve, when it is declared in the
 * barrel itself, when no specifier reaches its file, or when the module is
 * an installed package (in `node_modules`): those are never rewritten.
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
  const targets = declarationTargets(root, statement);
  if (targets.size === 0) return null;
  return retargetImport(statement, (binding) => targets.get(binding.local) ?? null);
}

/** Test-framework calls that name a module as their first argument. */
const MOCK_CALLS = new Set([
  "jest.mock",
  "jest.doMock",
  "jest.unmock",
  "jest.requireActual",
  "jest.requireMock",
  "vi.mock",
  "vi.doMock",
  "vi.unmock",
  "vi.importActual",
  "vi.importMock",
]);
/** Of those, the ones that replace a module with a mock. */
const MOCKING_CALLS = new Set(["jest.mock", "jest.doMock", "vi.mock", "vi.doMock"]);

/**
 * Point a test-framework mock (`jest.mock("./components")`,
 * `vi.mock("./lib", factory)`, `jest.requireActual(...)`, ...) at the modules
 * a barrel's bindings moved to. `call` is the call (from `root.importers()`
 * with `kind: "mock"`, or found by pattern); `to` the new specifiers.
 *
 * - An automatic mock (`jest.mock("m")`, no factory): the call stays (other
 *   files may still import the barrel, and mocking it is harmless) and one
 *   `jest.mock("<to>")` per module is added after it.
 * - A factory mock, `requireActual`, `importActual`, ...: with one module
 *   the specifier is replaced; with several it is left alone (null), because
 *   the factory's object would have to be split. Flag it for review.
 *
 * @returns One edit, or null when the call is not a mock or is left alone.
 * @example
 * ```ts
 * for (const mock of root.importers(barrel).filter((i) => i.kind === "mock")) {
 *   const edit = retargetMock(mock.node, ["./components/Button", "./components/Modal"]);
 * }
 * ```
 */
export function retargetMock<T extends Language>(call: SgNode<T>, to: string[]): Edit | null {
  const node = asTS(call);
  const callee = node.field("function")?.text() ?? "";
  if (!MOCK_CALLS.has(callee) || to.length === 0) return null;
  const args =
    node
      .field("arguments")
      ?.children()
      .filter((c) => c.isNamed()) ?? [];
  const specifier = args[0];
  if (!specifier || !(specifier.is("string") || specifier.is("template_string"))) return null;
  const quote = specifier.text()[0] ?? '"';
  const unique = [...new Set(to)];
  const automatic = MOCKING_CALLS.has(callee) && args.length === 1;
  if (!automatic) {
    return unique.length === 1 ? (specifier.replace(`${quote}${unique[0]}${quote}`) as Edit) : null;
  }
  const statement = node.parent();
  if (!statement?.is("expression_statement")) return null;
  const eol = call.getRoot().root().text().includes("\r\n") ? "\r\n" : "\n";
  const added = unique.map((module) => `${callee}(${quote}${module}${quote});`);
  return statement.replace([statement.text(), ...added].join(eol)) as Edit;
}

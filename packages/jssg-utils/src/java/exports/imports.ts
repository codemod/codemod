import { parse } from "codemod:ast-grep";
import type Java from "@codemod.com/jssg-types/langs/java";
import type { Edit, SgNode } from "@codemod.com/jssg-types/main";

export type JavaNode = SgNode<Java>;

export type ImportState = {
  imported: Set<string>;
  wildcardImportedPackages: Set<string>;
};

export type ImportCleanupOptions = {
  removeIfUnreferenced?: string[];
  addIfReferenced?: string[];
};

export function collectImports(rootNode: JavaNode): ImportState {
  const imported = new Set<string>();
  const wildcardImportedPackages = new Set<string>();

  for (const importNode of rootNode.findAll({ rule: { kind: "import_declaration" } })) {
    const importPath = getImportPath(importNode);
    if (!importPath) {
      continue;
    }

    if (importPath.endsWith(".*")) {
      wildcardImportedPackages.add(importPath.slice(0, -2));
    } else {
      imported.add(importPath);
    }
  }

  return { imported, wildcardImportedPackages };
}

export function isTypeImported(
  imports: ImportState,
  options: { simpleName: string; fullyQualifiedName: string },
): boolean {
  if (imports.imported.has(options.fullyQualifiedName)) {
    return true;
  }

  const packageName = packageNameOf(options.fullyQualifiedName);
  return imports.wildcardImportedPackages.has(packageName);
}

export function hasConflictingSimpleImport(
  imports: ImportState,
  options: { simpleName: string; expectedFullyQualifiedName: string },
): boolean {
  for (const importPath of imports.imported) {
    if (
      simpleName(importPath) === options.simpleName &&
      importPath !== options.expectedFullyQualifiedName
    ) {
      return true;
    }
  }

  return false;
}

export function cleanupImports(source: string, options: ImportCleanupOptions): string {
  const rootNode = parse<Java>("java", source).root();
  const edits = createImportCleanupEdits(rootNode, options, source);
  return edits.length > 0 ? normalizeImportSpacing(rootNode.commitEdits(edits)) : source;
}

export function createImportCleanupEdits(
  rootNode: JavaNode,
  options: ImportCleanupOptions,
  source = rootNode.text(),
): Edit[] {
  const edits: Edit[] = [];
  const removedImportIds = new Set<number>();
  const removeIfUnreferenced = new Set(options.removeIfUnreferenced ?? []);
  const addIfReferenced = options.addIfReferenced ?? [];

  for (const importNode of rootNode.findAll({ rule: { kind: "import_declaration" } })) {
    const importPath = getImportPath(importNode);
    if (!importPath || !removeIfUnreferenced.has(importPath)) {
      continue;
    }

    if (!referencesIdentifier(rootNode, simpleName(importPath))) {
      edits.push(expandRemovalEdit(importNode, source));
      removedImportIds.add(importNode.id());
    }
  }

  const importsToAdd = addIfReferenced
    .filter((importPath) => referencesIdentifier(rootNode, simpleName(importPath)))
    .filter((importPath) => !hasImport(rootNode, importPath))
    .sort();

  if (importsToAdd.length > 0) {
    addImportInsertionEdit(rootNode, importsToAdd, removedImportIds, edits);
  }

  return edits;
}

export function parseImportDeclaration(importText: string): string | null {
  const trimmed = importText.trim();
  if (!trimmed.startsWith("import ") || !trimmed.endsWith(";")) {
    return null;
  }

  return trimmed.slice("import ".length, -1).trim();
}

export function getImportPath(importNode: JavaNode): string | null {
  const scopedIdentifier = importNode.find({ rule: { kind: "scoped_identifier" } });
  if (!scopedIdentifier) {
    return null;
  }

  return importNode.find({ rule: { kind: "asterisk" } })
    ? `${scopedIdentifier.text()}.*`
    : scopedIdentifier.text();
}

export function simpleName(fullyQualifiedName: string): string {
  return fullyQualifiedName.slice(fullyQualifiedName.lastIndexOf(".") + 1);
}

export function referencesIdentifier(rootNode: JavaNode, name: string): boolean {
  return rootNode
    .findAll({
      rule: {
        any: [{ kind: "identifier" }, { kind: "type_identifier" }],
      },
    })
    .some((node) => node.text() === name && !isInsideImport(node));
}

function packageNameOf(fullyQualifiedName: string): string {
  return fullyQualifiedName.slice(0, fullyQualifiedName.lastIndexOf("."));
}

function hasImport(rootNode: JavaNode, importPath: string): boolean {
  return rootNode
    .findAll({ rule: { kind: "import_declaration" } })
    .some((importNode) => getImportPath(importNode) === importPath);
}

function addImportInsertionEdit(
  rootNode: JavaNode,
  importPaths: string[],
  removedImportIds: Set<number>,
  edits: Edit[],
): void {
  const importBlock = importPaths.map((path) => `import ${path};`).join("\n");
  const importNodes = rootNode.findAll({ rule: { kind: "import_declaration" } });
  const survivingImports = importNodes.filter((node) => !removedImportIds.has(node.id()));
  const lastImport = survivingImports[survivingImports.length - 1];

  if (lastImport) {
    edits.push({
      startPos: lastImport.range().end.index,
      endPos: lastImport.range().end.index,
      insertedText: `\n${importBlock}`,
    });
    return;
  }

  const firstRemovedImport = importNodes.find((node) => removedImportIds.has(node.id()));
  if (firstRemovedImport) {
    const removalEdit = edits.find(
      (edit) => edit.startPos === firstRemovedImport.range().start.index,
    );
    if (removalEdit) {
      removalEdit.insertedText = importBlock.endsWith("\n") ? importBlock : `${importBlock}\n`;
      return;
    }
  }

  const packageNode = rootNode.find({ rule: { kind: "package_declaration" } });
  if (packageNode) {
    edits.push({
      startPos: packageNode.range().end.index,
      endPos: packageNode.range().end.index,
      insertedText: `\n\n${importBlock}`,
    });
    return;
  }

  edits.push({
    startPos: 0,
    endPos: 0,
    insertedText: `${importBlock}\n`,
  });
}

function expandRemovalEdit(node: JavaNode, source: string): Edit {
  const range = node.range();
  let endPos = range.end.index;

  if (source[endPos] === "\r" && source[endPos + 1] === "\n") {
    endPos += 2;
  } else if (source[endPos] === "\n") {
    endPos += 1;
  }

  return {
    startPos: range.start.index,
    endPos,
    insertedText: "",
  };
}

function isInsideImport(node: JavaNode): boolean {
  return node.ancestors().some((ancestor) => ancestor.kind() === "import_declaration");
}

function normalizeImportSpacing(source: string): string {
  const lines = source.split("\n");
  const normalized: string[] = [];

  for (let index = 0; index < lines.length; index += 1) {
    const current = lines[index] ?? "";
    const previous = normalized[normalized.length - 1] ?? "";
    const next = lines[index + 1] ?? "";

    if (current === "" && previous === "") {
      continue;
    }

    if (current === "" && previous.startsWith("package ") && !next.startsWith("import ")) {
      normalized.push(current);
      continue;
    }

    normalized.push(current);
  }

  return normalized.join("\n");
}

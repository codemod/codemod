import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";

export type JavaNode = SgNode<Java>;

export type JavaImportState = {
  imported: Set<string>;
  wildcardImportedPackages: Set<string>;
};

export type JavaImportCleanupOptions = {
  removeIfUnreferenced?: string[];
  addIfReferenced?: string[];
};

export function collectJavaImports(rootNode: JavaNode): JavaImportState {
  const imported = new Set<string>();
  const wildcardImportedPackages = new Set<string>();

  for (const importNode of rootNode.findAll({ rule: { kind: "import_declaration" } })) {
    const importPath = parseJavaImportDeclaration(importNode.text());
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

export function isJavaTypeImported(
  imports: JavaImportState,
  options: { simpleName: string; fullyQualifiedName: string },
): boolean {
  if (imports.imported.has(options.fullyQualifiedName)) {
    return true;
  }

  const packageName = packageNameOf(options.fullyQualifiedName);
  return imports.wildcardImportedPackages.has(packageName);
}

export function hasConflictingJavaSimpleImport(
  imports: JavaImportState,
  options: { simpleName: string; expectedFullyQualifiedName: string },
): boolean {
  for (const importPath of imports.imported) {
    if (
      simpleJavaName(importPath) === options.simpleName &&
      importPath !== options.expectedFullyQualifiedName
    ) {
      return true;
    }
  }

  return false;
}

export function cleanupJavaImports(source: string, options: JavaImportCleanupOptions): string {
  const removeIfUnreferenced = new Set(options.removeIfUnreferenced ?? []);
  const addIfReferenced = options.addIfReferenced ?? [];
  const withoutImports = stripJavaImportLines(source);

  let next = source
    .split("\n")
    .filter((line) => {
      const importPath = parseJavaImportDeclaration(line);
      if (!importPath || !removeIfUnreferenced.has(importPath)) {
        return true;
      }

      return referencesJavaIdentifier(withoutImports, simpleJavaName(importPath));
    })
    .join("\n");

  const bodyWithoutImports = stripJavaImportLines(next);
  const importsToAdd = addIfReferenced
    .filter((importPath) =>
      referencesJavaIdentifier(bodyWithoutImports, simpleJavaName(importPath)),
    )
    .filter((importPath) => !hasJavaImportInSource(next, importPath))
    .sort();

  if (importsToAdd.length > 0) {
    next = insertJavaImportBlock(next, importsToAdd.map((path) => `import ${path};`).join("\n"));
  }

  return normalizeJavaImportSpacing(next);
}

export function parseJavaImportDeclaration(importText: string): string | null {
  const trimmed = importText.trim();
  if (!trimmed.startsWith("import ") || !trimmed.endsWith(";")) {
    return null;
  }

  return trimmed.slice("import ".length, -1).trim();
}

export function simpleJavaName(fullyQualifiedName: string): string {
  return fullyQualifiedName.slice(fullyQualifiedName.lastIndexOf(".") + 1);
}

export function referencesJavaIdentifier(source: string, name: string): boolean {
  let index = source.indexOf(name);
  while (index !== -1) {
    const previous = index === 0 ? "" : (source[index - 1] ?? "");
    const next = source[index + name.length] ?? "";
    if (!isJavaIdentifierPart(previous) && !isJavaIdentifierPart(next)) {
      return true;
    }
    index = source.indexOf(name, index + name.length);
  }

  return false;
}

function packageNameOf(fullyQualifiedName: string): string {
  return fullyQualifiedName.slice(0, fullyQualifiedName.lastIndexOf("."));
}

function stripJavaImportLines(source: string): string {
  return source
    .split("\n")
    .filter((line) => parseJavaImportDeclaration(line) === null)
    .join("\n");
}

function hasJavaImportInSource(source: string, importPath: string): boolean {
  return source.split("\n").some((line) => parseJavaImportDeclaration(line) === importPath);
}

function insertJavaImportBlock(source: string, importBlock: string): string {
  const lines = source.split("\n");
  let lastImportIndex = -1;
  let packageIndex = -1;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    if (parseJavaImportDeclaration(line) !== null) {
      lastImportIndex = index;
    }
    if (line.trim().startsWith("package ")) {
      packageIndex = index;
    }
  }

  const insertionIndex = lastImportIndex >= 0 ? lastImportIndex + 1 : packageIndex + 1;
  lines.splice(insertionIndex, 0, ...importBlock.split("\n"));
  return lines.join("\n");
}

function normalizeJavaImportSpacing(source: string): string {
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

function isJavaIdentifierPart(character: string): boolean {
  return /^[A-Za-z0-9_$]$/.test(character);
}

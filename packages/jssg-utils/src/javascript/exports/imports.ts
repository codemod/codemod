import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { SgNode } from "@codemod.com/jssg-types/main";
import { stringToExactRegexString } from "../../utils";

type GetImportOptions =
  | {
      type: "default";
      from: string;
    }
  | {
      type: "named";
      name: string;
      from: string;
    };

type GetImportResult<T extends Language> = {
  alias: string;
  isNamespace: boolean; // This means the import is like `import * as xyz from 'express';` which means we later need to access with `xyz.something`
  moduleType: "esm" | "cjs"; // "esm" for import statements (static or dynamic), "cjs" for require() calls
  node: SgNode<T, "identifier">;
} | null;

type Language = JS | TS | TSX;

/**
 * Determines whether an import match is ESM or CJS based on the AST node type.
 * - ESM: import statements (static or dynamic import())
 * - CJS: require() calls
 */
function getModuleType<T extends Language>(match: SgNode<T>): "esm" | "cjs" {
  const kind = match.kind();

  // ESM-only kinds (static imports)
  if (kind === "import_specifier" || kind === "import_statement") {
    return "esm";
  }

  const tsMatch = match as unknown as SgNode<TS>;

  // Find the variable_declarator (either the match itself or an ancestor)
  // For pattern matching variable_declarator directly, it's the match itself
  // For shorthand_property_identifier_pattern/pair_pattern, need to find ancestor
  let varDeclarator: SgNode<TS> | null = null;
  if (kind === "variable_declarator") {
    varDeclarator = tsMatch;
  } else {
    varDeclarator =
      (tsMatch
        .ancestors()
        .find((a) => a.kind() === "variable_declarator") as SgNode<TS>) ?? null;
  }

  if (!varDeclarator) {
    return "esm"; // Fallback for edge cases
  }

  // Check if the variable_declarator has a call_expression with function "require"
  const hasRequireCall = varDeclarator.has({
    rule: {
      kind: "call_expression",
      has: {
        field: "function",
        kind: "identifier",
        regex: "^require$",
      },
    },
  });

  return hasRequireCall ? "cjs" : "esm";
}

/**
 * Locate an import of a given module in a JS/TS program and return its alias/identifier node.
 *
 * The search supports multiple import styles for a specific source (options.from):
 * - Default ESM import: `import foo from "module"`
 * - Named ESM import: `import { bar as baz } from "module"`
 * - Bare ESM import: `import "module"`
 * - CommonJS: `const foo = require("module")` or `const { bar: baz } = require("module")`
 * - Dynamic import: `const foo = await import("module")`
 *
 * When options.type is "default", the function returns the identifier for the default import name
 * or the variable name bound from require/import calls. When options.type is "named", it returns
 * the identifier for the requested named specifier (using the alias if present).
 *
 * The returned object contains the resolved alias (the name to use at call sites), whether it was a
 * namespace import (currently always false in this implementation), and the underlying identifier node.
 *
 * @template T extends Language
 * @param program - The program node to search within.
 * @param options - Import lookup options. Use `{ type: "default", from }` for default/var forms, or `{ type: "named", name, from }` for a specific named specifier.
 * @returns The resolved import information or null if not found.
 */
export const getImport = <T extends Language>(
  program: SgNode<T, "program">,
  options: GetImportOptions
): GetImportResult<T> => {
  const tsProgram = program as unknown as SgNode<TS, "program">;

  const imports = tsProgram.findAll({
    rule: {
      any: [
        {
          all: [
            {
              kind: "import_specifier",
            },
            {
              has: {
                field: "alias",
                pattern: "$ALIAS",
              },
            },
            {
              has: {
                field: "name",
                pattern: "$ORIGINAL",
              },
            },
            {
              inside: {
                stopBy: "end",
                kind: "import_statement",
                has: {
                  field: "source",
                  pattern: "$SOURCE",
                },
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "import_statement",
            },
            {
              has: {
                kind: "import_clause",
                has: {
                  kind: "identifier",
                  pattern: "$DEFAULT_NAME",
                },
              },
            },
            {
              has: {
                field: "source",
                pattern: "$SOURCE",
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "import_specifier",
            },
            {
              has: {
                field: "name",
                pattern: "$ORIGINAL",
              },
            },
            {
              inside: {
                stopBy: "end",
                kind: "import_statement",
                has: {
                  field: "source",
                  pattern: "$SOURCE",
                },
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "variable_declarator",
            },
            {
              has: {
                field: "name",
                kind: "identifier",
                pattern: "$VAR_NAME",
              },
            },
            {
              has: {
                field: "value",
                any: [
                  {
                    all: [
                      {
                        kind: "call_expression",
                      },
                      {
                        has: {
                          field: "function",
                          regex: "^(require|import)$",
                        },
                      },
                      {
                        has: {
                          field: "arguments",
                          has: {
                            kind: "string",
                            pattern: "$SOURCE",
                          },
                        },
                      },
                    ],
                  },
                  {
                    kind: "await_expression",
                    has: {
                      all: [
                        {
                          kind: "call_expression",
                        },
                        {
                          has: {
                            field: "function",
                            regex: "^(require|import)$",
                          },
                        },
                        {
                          has: {
                            field: "arguments",
                            has: {
                              kind: "string",
                              pattern: "$SOURCE",
                            },
                          },
                        },
                      ],
                    },
                  },
                ],
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "shorthand_property_identifier_pattern",
            },
            {
              pattern: "$ORIGINAL",
            },
            {
              inside: {
                kind: "object_pattern",
                inside: {
                  kind: "variable_declarator",
                  has: {
                    field: "value",
                    any: [
                      {
                        all: [
                          {
                            kind: "call_expression",
                          },
                          {
                            has: {
                              field: "function",
                              regex: "^(require|import)$",
                            },
                          },
                          {
                            has: {
                              field: "arguments",
                              has: {
                                kind: "string",
                                pattern: "$SOURCE",
                              },
                            },
                          },
                        ],
                      },
                      {
                        kind: "await_expression",
                        has: {
                          all: [
                            {
                              kind: "call_expression",
                            },
                            {
                              has: {
                                field: "function",
                                regex: "^(require|import)$",
                              },
                            },
                            {
                              has: {
                                field: "arguments",
                                has: {
                                  kind: "string",
                                  pattern: "$SOURCE",
                                },
                              },
                            },
                          ],
                        },
                      },
                    ],
                  },
                  stopBy: "end",
                },
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "pair_pattern",
            },
            {
              has: {
                field: "key",
                kind: "property_identifier",
                pattern: "$ORIGINAL",
              },
            },
            {
              has: {
                field: "value",
                kind: "identifier",
                pattern: "$ALIAS",
              },
            },
            {
              inside: {
                kind: "object_pattern",
                inside: {
                  kind: "variable_declarator",
                  has: {
                    field: "value",
                    any: [
                      {
                        all: [
                          {
                            kind: "call_expression",
                          },
                          {
                            has: {
                              field: "function",
                              regex: "^(require|import)$",
                            },
                          },
                          {
                            has: {
                              field: "arguments",
                              has: {
                                kind: "string",
                                pattern: "$SOURCE",
                              },
                            },
                          },
                        ],
                      },
                      {
                        kind: "await_expression",
                        has: {
                          all: [
                            {
                              kind: "call_expression",
                            },
                            {
                              has: {
                                field: "function",
                                regex: "^(require|import)$",
                              },
                            },
                            {
                              has: {
                                field: "arguments",
                                has: {
                                  kind: "string",
                                  pattern: "$SOURCE",
                                },
                              },
                            },
                          ],
                        },
                      },
                    ],
                  },
                  stopBy: "end",
                },
                stopBy: "end",
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "string",
            },
            {
              pattern: "$SOURCE",
            },
            {
              inside: {
                kind: "arguments",
                inside: {
                  kind: "call_expression",
                  has: {
                    field: "function",
                    regex: "^(require|import)$",
                  },
                },
                stopBy: "end",
              },
            },
            {
              not: {
                inside: {
                  kind: "lexical_declaration",
                  stopBy: "end",
                },
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "import_statement",
            },
            {
              has: {
                kind: "import_clause",
                has: {
                  kind: "namespace_import",
                  has: {
                    kind: "identifier",
                    pattern: "$NAMESPACE_ALIAS",
                  },
                },
              },
            },
            {
              has: {
                field: "source",
                pattern: "$SOURCE",
              },
            },
          ],
        },
        {
          all: [
            {
              kind: "import_statement",
            },
            {
              not: {
                has: {
                  kind: "import_clause",
                },
              },
            },
            {
              has: {
                field: "source",
                pattern: "$SOURCE",
              },
            },
          ],
        },
      ],
    },
    constraints: {
      SOURCE: {
        any: [
          {
            regex: stringToExactRegexString(options.from),
          },
          {
            has: {
              kind: "string_fragment",
              regex: stringToExactRegexString(options.from),
            },
          },
        ],
      },
    },
  });

  if (imports.length === 0) {
    return null;
  }

  if (options.type === "default") {
    const foundMatch = imports.find((m) => {
      return !!(m.getMatch("DEFAULT_NAME") ?? m.getMatch("VAR_NAME"));
    });
    if (foundMatch) {
      const defaultName = foundMatch.getMatch("DEFAULT_NAME");
      const varName = foundMatch.getMatch("VAR_NAME");
      const name = defaultName?.text() ?? varName?.text() ?? "";
      return {
        alias: name,
        isNamespace: false,
        moduleType: getModuleType(foundMatch),
        node: (defaultName ?? varName)! as unknown as SgNode<T, "identifier">,
      };
    }
  }

  if (options.type === "named") {
    const foundMatch = imports.find((m) => {
      return m.getMatch("ORIGINAL")?.text() === options.name;
    });

    if (foundMatch) {
      const original = foundMatch.getMatch("ORIGINAL");
      const alias = foundMatch.getMatch("ALIAS");
      return {
        alias: alias?.text() ?? original?.text() ?? "",
        isNamespace: false,
        moduleType: getModuleType(foundMatch),
        node: (alias ?? original)! as unknown as SgNode<T, "identifier">,
      };
    }
  }

  const namespaceImport = imports.find((m) => {
    return m.getMatch("NAMESPACE_ALIAS");
  });
  if (namespaceImport) {
    return {
      alias: namespaceImport.getMatch("NAMESPACE_ALIAS")?.text() ?? "",
      isNamespace: true,
      moduleType: "esm" as const, // Namespace imports are always ESM
      node: namespaceImport.getMatch("NAMESPACE_ALIAS")! as unknown as SgNode<
        T,
        "identifier"
      >,
    };
  }

  return null;
};

// ============================================================================
// Import Manipulation Types
// ============================================================================

type ImportSpecifier = { name: string; alias?: string };

type AddImportOptions =
  | { type: "default"; name: string; from: string; moduleType?: "esm" | "cjs" }
  | { type: "namespace"; name: string; from: string }
  | {
      type: "named";
      specifiers: ImportSpecifier[];
      from: string;
      moduleType?: "esm" | "cjs";
    };

type RemoveImportOptions =
  | { type: "default"; from: string }
  | { type: "namespace"; from: string }
  | { type: "named"; specifiers: string[]; from: string };

interface Edit {
  startPos: number;
  endPos: number;
  insertedText: string;
}

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Find all import statements and require declarations in the program.
 */
function findAllImportStatements<T extends Language>(
  program: SgNode<T, "program">
): SgNode<T>[] {
  const tsProgram = program as unknown as SgNode<TS, "program">;

  // Find ESM import statements
  const esmImports = tsProgram.findAll({
    rule: { kind: "import_statement" },
  });

  // Find CJS require declarations (const x = require(...))
  const cjsImports = tsProgram.findAll({
    rule: {
      kind: "lexical_declaration",
      has: {
        kind: "variable_declarator",
        has: {
          field: "value",
          any: [
            {
              kind: "call_expression",
              has: {
                field: "function",
                kind: "identifier",
                regex: "^require$",
              },
            },
            {
              kind: "await_expression",
              has: {
                kind: "call_expression",
                has: {
                  field: "function",
                  regex: "^import$",
                },
              },
            },
          ],
        },
      },
    },
  });

  return [...esmImports, ...cjsImports] as unknown as SgNode<T>[];
}

/**
 * Find an existing ESM import statement from a specific source.
 */
function findExistingEsmImport<T extends Language>(
  program: SgNode<T, "program">,
  source: string
): SgNode<T, "import_statement"> | null {
  const tsProgram = program as unknown as SgNode<TS, "program">;

  const importStmt = tsProgram.find({
    rule: {
      kind: "import_statement",
      has: {
        field: "source",
        has: {
          kind: "string_fragment",
          regex: stringToExactRegexString(source),
        },
      },
    },
  });

  return importStmt as unknown as SgNode<T, "import_statement"> | null;
}

/**
 * Find the named_imports node within an import statement.
 */
function findNamedImports<T extends Language>(
  importStmt: SgNode<T>
): SgNode<T> | null {
  const tsImport = importStmt as unknown as SgNode<TS>;
  const namedImports = tsImport.find({
    rule: { kind: "named_imports" },
  });
  return namedImports as unknown as SgNode<T> | null;
}

/**
 * Generate a specifier string like "foo" or "foo as bar".
 */
function formatSpecifier(spec: ImportSpecifier): string {
  if (spec.alias && spec.alias !== spec.name) {
    return `${spec.name} as ${spec.alias}`;
  }
  return spec.name;
}

/**
 * Generate ESM import statement text.
 */
function generateEsmImport(options: AddImportOptions): string {
  const source = options.from;

  if (options.type === "default") {
    return `import ${options.name} from '${source}';\n`;
  }

  if (options.type === "namespace") {
    return `import * as ${options.name} from '${source}';\n`;
  }

  // Named imports
  const specifierStr = options.specifiers.map(formatSpecifier).join(", ");
  return `import { ${specifierStr} } from '${source}';\n`;
}

/**
 * Generate CJS require statement text.
 */
function generateCjsRequire(options: AddImportOptions): string {
  const source = options.from;

  if (options.type === "default") {
    return `const ${options.name} = require('${source}');\n`;
  }

  if (options.type === "namespace") {
    // Namespace-like for CJS
    return `const ${options.name} = require('${source}');\n`;
  }

  // Named imports (destructured require)
  const specifierStr = options.specifiers
    .map((spec) => {
      if (spec.alias && spec.alias !== spec.name) {
        return `${spec.name}: ${spec.alias}`;
      }
      return spec.name;
    })
    .join(", ");
  return `const { ${specifierStr} } = require('${source}');\n`;
}

// ============================================================================
// addImport
// ============================================================================

/**
 * Add an import to the program. Smart behavior:
 * - Skip if the import already exists
 * - Merge into existing import statement for named imports
 * - Create new import statement otherwise
 *
 * @returns Edit to apply, or null if import already exists
 */
export function addImport<T extends Language>(
  program: SgNode<T, "program">,
  options: AddImportOptions
): Edit | null {
  const moduleType =
    options.type === "namespace"
      ? "esm"
      : (options.moduleType ?? "esm");

  // Check if import already exists
  if (options.type === "default") {
    const existing = getImport(program, { type: "default", from: options.from });
    if (existing && !existing.isNamespace) {
      return null; // Already has default import
    }
  } else if (options.type === "namespace") {
    const existing = getImport(program, { type: "default", from: options.from });
    if (existing && existing.isNamespace) {
      return null; // Already has namespace import
    }
  } else if (options.type === "named") {
    // Filter out specifiers that already exist
    const newSpecifiers: ImportSpecifier[] = [];
    for (const spec of options.specifiers) {
      const existing = getImport(program, {
        type: "named",
        name: spec.name,
        from: options.from,
      });
      if (!existing) {
        newSpecifiers.push(spec);
      }
    }

    if (newSpecifiers.length === 0) {
      return null; // All specifiers already exist
    }

    // For ESM named imports, try to merge into existing import
    if (moduleType === "esm") {
      const existingImport = findExistingEsmImport(program, options.from);
      if (existingImport) {
        const namedImports = findNamedImports(existingImport);
        if (namedImports) {
          // Add to existing named_imports: insert before the closing brace
          const namedImportsText = namedImports.text();
          const closingBraceIdx = namedImportsText.lastIndexOf("}");
          if (closingBraceIdx > 0) {
            const insertPos =
              namedImports.range().start.index + closingBraceIdx;
            const specifierStr = newSpecifiers.map(formatSpecifier).join(", ");
            // Check if there are existing specifiers (need comma)
            const hasExistingSpecifiers =
              namedImportsText.slice(1, closingBraceIdx).trim().length > 0;
            const insertText = hasExistingSpecifiers
              ? `, ${specifierStr}`
              : ` ${specifierStr} `;

            return {
              startPos: insertPos,
              endPos: insertPos,
              insertedText: insertText,
            };
          }
        } else {
          // Import exists but has no named_imports (e.g., default import only)
          // Add named imports to it: import foo from 'mod' -> import foo, { bar } from 'mod'
          const importClause = (existingImport as unknown as SgNode<TS>).find({
            rule: { kind: "import_clause" },
          });
          if (importClause) {
            const specifierStr = newSpecifiers.map(formatSpecifier).join(", ");
            const insertPos = importClause.range().end.index;
            return {
              startPos: insertPos,
              endPos: insertPos,
              insertedText: `, { ${specifierStr} }`,
            };
          }
        }
      }
    }

    // Update options with filtered specifiers for new import creation
    options = { ...options, specifiers: newSpecifiers };
  }

  // Find insertion position (after last import, or at file start)
  const allImports = findAllImportStatements(program);
  let insertPos = 0;
  let prefix = "";

  const lastImport = allImports[allImports.length - 1];
  if (lastImport) {
    insertPos = lastImport.range().end.index;
    // Check if there's a newline after the last import
    const programText = program.text();
    if (programText[insertPos] !== "\n") {
      prefix = "\n";
    }
  }

  // Generate import text
  const importText =
    moduleType === "esm"
      ? generateEsmImport(options)
      : generateCjsRequire(options);

  return {
    startPos: insertPos,
    endPos: insertPos,
    insertedText: prefix + importText,
  };
}

// ============================================================================
// removeImport
// ============================================================================

/**
 * Find the import statement containing a specific specifier.
 */
function findImportStatementForSpecifier<T extends Language>(
  program: SgNode<T, "program">,
  specifierName: string,
  source: string
): { statement: SgNode<T>; specifier: SgNode<T> } | null {
  const tsProgram = program as unknown as SgNode<TS, "program">;

  // Find ESM import specifier
  const esmSpecifier = tsProgram.find({
    rule: {
      kind: "import_specifier",
      has: {
        field: "name",
        regex: stringToExactRegexString(specifierName),
      },
      inside: {
        kind: "import_statement",
        has: {
          field: "source",
          any: [
            { regex: stringToExactRegexString(source) },
            {
              has: {
                kind: "string_fragment",
                regex: stringToExactRegexString(source),
              },
            },
          ],
        },
        stopBy: "end",
      },
    },
  });

  if (esmSpecifier) {
    // Find the parent import_statement
    const importStmt = esmSpecifier
      .ancestors()
      .find((a) => a.kind() === "import_statement");
    if (importStmt) {
      return {
        statement: importStmt as unknown as SgNode<T>,
        specifier: esmSpecifier as unknown as SgNode<T>,
      };
    }
  }

  // Find CJS destructured require
  const cjsSpecifier = tsProgram.find({
    rule: {
      any: [
        {
          kind: "shorthand_property_identifier_pattern",
          regex: stringToExactRegexString(specifierName),
          inside: {
            kind: "object_pattern",
            inside: {
              kind: "variable_declarator",
              has: {
                field: "value",
                all: [
                  { kind: "call_expression" },
                  {
                    has: {
                      field: "function",
                      kind: "identifier",
                      regex: "^require$",
                    },
                  },
                  {
                    has: {
                      field: "arguments",
                      has: {
                        kind: "string",
                        any: [
                          { regex: stringToExactRegexString(source) },
                          {
                            has: {
                              kind: "string_fragment",
                              regex: stringToExactRegexString(source),
                            },
                          },
                        ],
                      },
                    },
                  },
                ],
              },
              stopBy: "end",
            },
            stopBy: "end",
          },
        },
        {
          kind: "pair_pattern",
          has: {
            field: "key",
            regex: stringToExactRegexString(specifierName),
          },
          inside: {
            kind: "object_pattern",
            inside: {
              kind: "variable_declarator",
              has: {
                field: "value",
                all: [
                  { kind: "call_expression" },
                  {
                    has: {
                      field: "function",
                      kind: "identifier",
                      regex: "^require$",
                    },
                  },
                  {
                    has: {
                      field: "arguments",
                      has: {
                        kind: "string",
                        any: [
                          { regex: stringToExactRegexString(source) },
                          {
                            has: {
                              kind: "string_fragment",
                              regex: stringToExactRegexString(source),
                            },
                          },
                        ],
                      },
                    },
                  },
                ],
              },
              stopBy: "end",
            },
            stopBy: "end",
          },
        },
      ],
    },
  });

  if (cjsSpecifier) {
    // Find the parent lexical_declaration
    const lexicalDecl = cjsSpecifier
      .ancestors()
      .find((a) => a.kind() === "lexical_declaration");
    if (lexicalDecl) {
      return {
        statement: lexicalDecl as unknown as SgNode<T>,
        specifier: cjsSpecifier as unknown as SgNode<T>,
      };
    }
  }

  return null;
}

/**
 * Count the number of specifiers in an import statement.
 */
function countSpecifiersInStatement<T extends Language>(
  statement: SgNode<T>
): number {
  const tsStmt = statement as unknown as SgNode<TS>;

  if (tsStmt.kind() === "import_statement") {
    return tsStmt.findAll({ rule: { kind: "import_specifier" } }).length;
  }

  // CJS: count in object_pattern
  const objectPattern = tsStmt.find({ rule: { kind: "object_pattern" } });
  if (objectPattern) {
    const shorthand = objectPattern.findAll({
      rule: { kind: "shorthand_property_identifier_pattern" },
    });
    const pairs = objectPattern.findAll({ rule: { kind: "pair_pattern" } });
    return shorthand.length + pairs.length;
  }

  return 0;
}

/**
 * Find the full range of a statement including leading/trailing whitespace.
 */
function getStatementRangeWithNewline<T extends Language>(
  statement: SgNode<T>,
  programText: string
): { start: number; end: number } {
  const range = statement.range();
  let end = range.end.index;

  // Include trailing newline if present
  if (programText[end] === "\n") {
    end++;
  }

  return { start: range.start.index, end };
}

/**
 * Find the range of a specifier including comma/whitespace for clean removal.
 */
function getSpecifierRangeWithSeparator<T extends Language>(
  specifier: SgNode<T>,
  programText: string
): { start: number; end: number } {
  const range = specifier.range();
  let start = range.start.index;
  let end = range.end.index;

  // Check for trailing comma and whitespace
  let i = end;
  while (i < programText.length && /\s/.test(programText[i] ?? "")) {
    i++;
  }
  if (i < programText.length && programText[i] === ",") {
    end = i + 1;
    // Also consume whitespace after comma
    while (end < programText.length && /\s/.test(programText[end] ?? "")) {
      end++;
    }
  } else {
    // Check for leading comma (if this is the last specifier)
    let j = start - 1;
    while (j >= 0 && /\s/.test(programText[j] ?? "")) {
      j--;
    }
    if (j >= 0 && programText[j] === ",") {
      start = j;
    }
  }

  return { start, end };
}

/**
 * Remove an import from the program. Smart behavior:
 * - Default/namespace: Removes entire import statement
 * - Named (multiple specifiers exist): Removes only the specified specifiers
 * - Named (removing last specifiers): Removes entire import statement
 *
 * @returns Edit to apply, or null if import not found
 */
export function removeImport<T extends Language>(
  program: SgNode<T, "program">,
  options: RemoveImportOptions
): Edit | null {
  const programText = program.text();

  if (options.type === "default") {
    // Find default import and remove entire statement
    const existing = getImport(program, { type: "default", from: options.from });
    if (!existing || existing.isNamespace) {
      return null;
    }

    // Find the import statement containing this default import
    const tsProgram = program as unknown as SgNode<TS, "program">;
    const importStmt = tsProgram.find({
      rule: {
        kind: "import_statement",
        all: [
          {
            has: {
              kind: "import_clause",
              has: {
                kind: "identifier",
                regex: stringToExactRegexString(existing.alias),
              },
            },
          },
          {
            has: {
              field: "source",
              any: [
                { regex: stringToExactRegexString(options.from) },
                {
                  has: {
                    kind: "string_fragment",
                    regex: stringToExactRegexString(options.from),
                  },
                },
              ],
            },
          },
        ],
      },
    });

    if (importStmt) {
      const { start, end } = getStatementRangeWithNewline(
        importStmt as unknown as SgNode<T>,
        programText
      );
      return { startPos: start, endPos: end, insertedText: "" };
    }

    // Check for CJS require
    const cjsDecl = tsProgram.find({
      rule: {
        kind: "lexical_declaration",
        has: {
          kind: "variable_declarator",
          all: [
            {
              has: {
                field: "name",
                kind: "identifier",
                regex: stringToExactRegexString(existing.alias),
              },
            },
            {
              has: {
                field: "value",
                any: [
                  {
                    kind: "call_expression",
                    has: {
                      field: "function",
                      kind: "identifier",
                      regex: "^require$",
                    },
                  },
                  {
                    kind: "await_expression",
                    has: {
                      kind: "call_expression",
                    },
                  },
                ],
              },
            },
          ],
        },
      },
    });

    if (cjsDecl) {
      const { start, end } = getStatementRangeWithNewline(
        cjsDecl as unknown as SgNode<T>,
        programText
      );
      return { startPos: start, endPos: end, insertedText: "" };
    }

    return null;
  }

  if (options.type === "namespace") {
    // Find namespace import and remove entire statement
    const existing = getImport(program, { type: "default", from: options.from });
    if (!existing || !existing.isNamespace) {
      return null;
    }

    const tsProgram = program as unknown as SgNode<TS, "program">;
    const importStmt = tsProgram.find({
      rule: {
        kind: "import_statement",
        all: [
          {
            has: {
              kind: "import_clause",
              has: {
                kind: "namespace_import",
                has: {
                  kind: "identifier",
                  regex: stringToExactRegexString(existing.alias),
                },
              },
            },
          },
          {
            has: {
              field: "source",
              any: [
                { regex: stringToExactRegexString(options.from) },
                {
                  has: {
                    kind: "string_fragment",
                    regex: stringToExactRegexString(options.from),
                  },
                },
              ],
            },
          },
        ],
      },
    });

    if (importStmt) {
      const { start, end } = getStatementRangeWithNewline(
        importStmt as unknown as SgNode<T>,
        programText
      );
      return { startPos: start, endPos: end, insertedText: "" };
    }

    return null;
  }

  // Named imports: remove specific specifiers
  if (options.type === "named") {
    // Find the first specifier to remove
    for (const specName of options.specifiers) {
      const found = findImportStatementForSpecifier(
        program,
        specName,
        options.from
      );

      if (found) {
        const specifierCount = countSpecifiersInStatement(found.statement);

        // If this is the last specifier, remove the entire statement
        if (specifierCount <= options.specifiers.length) {
          const { start, end } = getStatementRangeWithNewline(
            found.statement,
            programText
          );
          return { startPos: start, endPos: end, insertedText: "" };
        }

        // Otherwise, just remove this specifier
        const { start, end } = getSpecifierRangeWithSeparator(
          found.specifier,
          programText
        );
        return { startPos: start, endPos: end, insertedText: "" };
      }
    }

    return null;
  }

  return null;
}

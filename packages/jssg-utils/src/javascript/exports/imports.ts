import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { Rule, SgNode } from "@codemod.com/jssg-types/main";
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

type ResolvedImport<T extends Language> = NonNullable<GetImportResult<T>>;

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
  | {
      type: "default";
      from: string;
      /**
       * When `getImport` finds no binding, also remove side-effect-only lines:
       * `import 'module'` and `require('module');`. Default false.
       */
      removeSideEffectForms?: boolean;
    }
  | { type: "namespace"; from: string }
  | { type: "named"; specifiers: string[]; from: string };

interface Edit {
  startPos: number;
  endPos: number;
  insertedText: string;
}

const CJS_OR_DYNAMIC_VALUE_UTIL_RULE = "is-cjs-or-dynamic-value" as const;

// ============================================================================
// Utils
// ============================================================================

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
      (tsMatch.ancestors().find((a) => a.kind() === "variable_declarator") as SgNode<TS>) ?? null;
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
 * Run the full import-pattern query for a given source and return all raw AST matches.
 */
function findRawImportMatches<T extends Language>(
  program: SgNode<T, "program">,
  from: string,
): SgNode<TS>[] {
  const tsProgram = program as unknown as SgNode<TS, "program">;
  const cjsOrDynamicValue = {
    any: [
      {
        all: [
          { kind: "call_expression" },
          { has: { field: "function", regex: "^(require|import)$" } },
          {
            has: {
              field: "arguments",
              has: { kind: "string", pattern: "$SOURCE" },
            },
          },
        ],
      },
      {
        kind: "await_expression",
        has: {
          all: [
            { kind: "call_expression" },
            { has: { field: "function", regex: "^(require|import)$" } },
            {
              has: {
                field: "arguments",
                has: { kind: "string", pattern: "$SOURCE" },
              },
            },
          ],
        },
      },
    ],
  } satisfies Rule<Language>;

  return tsProgram.findAll({
    utils: {
      [CJS_OR_DYNAMIC_VALUE_UTIL_RULE]: cjsOrDynamicValue,
    },
    rule: {
      any: [
        // - ESM: named with alias: import { foo as bar } from "mod"
        {
          all: [
            { kind: "import_specifier" },
            { has: { field: "alias", pattern: "$ALIAS" } },
            { has: { field: "name", pattern: "$ORIGINAL" } },
            {
              inside: {
                stopBy: "end",
                kind: "import_statement",
                has: { field: "source", pattern: "$SOURCE" },
              },
            },
          ],
        },
        // - ESM: default import: import foo from "mod"
        {
          all: [
            { kind: "import_statement" },
            {
              has: {
                kind: "import_clause",
                has: { kind: "identifier", pattern: "$DEFAULT_NAME" },
              },
            },
            { has: { field: "source", pattern: "$SOURCE" } },
          ],
        },
        // - ESM: named without alias: import { foo } from "mod"
        {
          all: [
            { kind: "import_specifier" },
            { has: { field: "name", pattern: "$ORIGINAL" } },
            {
              inside: {
                stopBy: "end",
                kind: "import_statement",
                has: { field: "source", pattern: "$SOURCE" },
              },
            },
          ],
        },
        // - CJS/dynamic: const foo = require("mod") / import("mod")
        {
          all: [
            { kind: "variable_declarator" },
            { has: { field: "name", kind: "identifier", pattern: "$VAR_NAME" } },
            {
              has: {
                field: "value",
                matches: CJS_OR_DYNAMIC_VALUE_UTIL_RULE,
              },
            },
          ],
        },
        // - CJS: const { foo } = require("mod")
        {
          all: [
            { kind: "shorthand_property_identifier_pattern" },
            { pattern: "$ORIGINAL" },
            {
              inside: {
                kind: "object_pattern",
                inside: {
                  stopBy: "end",
                  kind: "variable_declarator",
                  has: {
                    field: "value",
                    matches: CJS_OR_DYNAMIC_VALUE_UTIL_RULE,
                  },
                },
              },
            },
          ],
        },
        // - CJS: const { foo: bar } = require("mod")
        {
          all: [
            { kind: "pair_pattern" },
            { has: { field: "key", kind: "property_identifier", pattern: "$ORIGINAL" } },
            { has: { field: "value", kind: "identifier", pattern: "$ALIAS" } },
            {
              inside: {
                stopBy: "end",
                kind: "object_pattern",
                inside: {
                  stopBy: "end",
                  kind: "variable_declarator",
                  has: {
                    field: "value",
                    matches: CJS_OR_DYNAMIC_VALUE_UTIL_RULE,
                  },
                },
              },
            },
          ],
        },
        // - Bare require/import (no binding): require("mod")
        {
          all: [
            { kind: "string" },
            { pattern: "$SOURCE" },
            {
              inside: {
                stopBy: "end",
                kind: "arguments",
                inside: {
                  kind: "call_expression",
                  has: { field: "function", regex: "^(require|import)$" },
                },
              },
            },
            { not: { inside: { stopBy: "end", kind: "lexical_declaration" } } },
          ],
        },
        // - ESM namespace: import * as foo from "mod"
        {
          all: [
            { kind: "import_statement" },
            {
              has: {
                kind: "import_clause",
                has: {
                  kind: "namespace_import",
                  has: { kind: "identifier", pattern: "$NAMESPACE_ALIAS" },
                },
              },
            },
            { has: { field: "source", pattern: "$SOURCE" } },
          ],
        },
        // - ESM bare: import "mod"
        {
          all: [
            { kind: "import_statement" },
            { not: { has: { kind: "import_clause" } } },
            { has: { field: "source", pattern: "$SOURCE" } },
          ],
        },
      ],
    },
    constraints: {
      SOURCE: {
        any: [
          { regex: stringToExactRegexString(from) },
          {
            has: {
              kind: "string_fragment",
              regex: stringToExactRegexString(from),
            },
          },
        ],
      },
    },
  });
}

/**
 * Attempt to interpret a single raw AST match against the requested import type.
 * Returns a resolved import if the match is relevant to `options`, null otherwise.
 *
 * Namespace imports are NOT handled here - use `matchToNamespaceImport(s)()` for those.
 */
function matchToImportResult<T extends Language>(
  match: SgNode<TS>,
  options: GetImportOptions,
): ResolvedImport<T> | null {
  if (options.type === "default") {
    const nameNode = match.getMatch("DEFAULT_NAME") ?? match.getMatch("VAR_NAME");
    if (!nameNode) return null;
    return {
      alias: nameNode.text(),
      isNamespace: false,
      moduleType: getModuleType(match),
      node: nameNode as unknown as SgNode<T, "identifier">,
    };
  }

  if (options.type === "named") {
    const original = match.getMatch("ORIGINAL");
    if (original?.text() !== options.name) return null;
    const alias = match.getMatch("ALIAS");
    return {
      alias: alias?.text() ?? original?.text() ?? "",
      isNamespace: false,
      moduleType: getModuleType(match),
      node: (alias ?? original)! as unknown as SgNode<T, "identifier">,
    };
  }

  return null;
}

/**
 * Extract ALL namespace import results from a list of raw matches.
 */
function matchToNamespaceImports<T extends Language>(matches: SgNode<TS>[]): ResolvedImport<T>[] {
  return matches
    .filter((m) => m.getMatch("NAMESPACE_ALIAS"))
    .map((m) => {
      const aliasNode = m.getMatch("NAMESPACE_ALIAS")!;
      return {
        alias: aliasNode.text(),
        isNamespace: true,
        moduleType: "esm" as const, // Namespace imports are always ESM
        node: aliasNode as unknown as SgNode<T, "identifier">,
      };
    });
}

/**
 * Extract the first namespace import result from a list of raw matches, if one exists.
 */
function matchToNamespaceImport<T extends Language>(
  matches: SgNode<TS>[],
): ResolvedImport<T> | null {
  return matchToNamespaceImports<T>(matches)[0] ?? null;
}

/**
 * Find all import statements and require declarations in the program.
 * Single source of truth for locating import-level AST nodes — used by both
 * addImport (insertion point) and removeImport (statement lookup).
 *
 * Only single-declarator CJS declarations are included. Multi-declarator
 * declarations (e.g. `const foo = require('mod'), x = 1`) are excluded to
 * prevent removeImport from accidentally deleting unrelated bindings when it
 * removes the whole statement. Such declarations return null from removeImport.
 */
function findAllImportStatements<T extends Language>(program: SgNode<T, "program">): SgNode<T>[] {
  const tsProgram = program as unknown as SgNode<TS, "program">;

  // Find ESM import statements
  const esmImports = tsProgram.findAll({
    rule: { kind: "import_statement" },
  });

  // Find CJS require declarations (`const`/`let` and `var`: x = require(...))
  // Restricted to single-declarator declarations only - if a second variable_declarator
  // exists in the same declaration, we skip it entirely rather than risk removing
  // unrelated bindings during whole-statement removal.
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
      // Guard: skip declarations with more than one declarator
      not: {
        has: {
          kind: "variable_declarator",
          nthChild: 2,
        },
      },
    },
  });

  // `var foo = require(...)` uses variable_declaration, not lexical_declaration
  const cjsVarImports = tsProgram.findAll({
    rule: {
      kind: "variable_declaration",
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
      not: {
        has: {
          kind: "variable_declarator",
          nthChild: 2,
        },
      },
    },
  });

  return [...esmImports, ...cjsImports, ...cjsVarImports] as unknown as SgNode<T>[];
}

/**
 * Find an existing ESM import statement from a specific source.
 */
function findExistingEsmImport<T extends Language>(
  program: SgNode<T, "program">,
  source: string,
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
function findNamedImports<T extends Language>(importStmt: SgNode<T>): SgNode<T> | null {
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

/**
 * Find the import statement containing a specific specifier.
 */
function findImportStatementForSpecifier<T extends Language>(
  program: SgNode<T, "program">,
  specifierName: string,
  source: string,
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
    const importStmt = esmSpecifier.ancestors().find((a) => a.kind() === "import_statement");
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
    const lexicalDecl = cjsSpecifier.ancestors().find((a) => a.kind() === "lexical_declaration");
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
function countSpecifiersInStatement<T extends Language>(statement: SgNode<T>): number {
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
 * Find the full range of a statement including one trailing line ending, if present.
 * Handles CRLF, LF, and legacy lone CR so removal edits stay consistent across platforms.
 */
function getStatementRangeWithNewline<T extends Language>(
  statement: SgNode<T>,
  programText: string,
): { start: number; end: number } {
  const range = statement.range();
  let end = range.end.index;

  if (programText.slice(end, end + 2) === "\r\n") {
    end += 2;
  } else if (programText[end] === "\n") {
    end++;
  } else if (programText[end] === "\r") {
    end++;
  }

  return { start: range.start.index, end };
}

/**
 * Find the range of a specifier including comma/whitespace for clean removal.
 */
function getSpecifierRangeWithSeparator<T extends Language>(
  specifier: SgNode<T>,
  programText: string,
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

// ============================================================================
// Public API
// ============================================================================

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
 * namespace import, and the underlying identifier node.
 *
 * @template T extends Language
 * @param program - The program node to search within.
 * @param options - Import lookup options. Use `{ type: "default", from }` for default/var forms, or `{ type: "named", name, from }` for a specific named specifier.
 * @returns The resolved import information or null if not found.
 */
export const getImport = <T extends Language>(
  program: SgNode<T, "program">,
  options: GetImportOptions,
): GetImportResult<T> => {
  const matches = findRawImportMatches(program, options.from);
  if (matches.length === 0) return null;

  for (const match of matches) {
    const result = matchToImportResult<T>(match, options);
    if (result) return result;
  }

  return matchToNamespaceImport<T>(matches);
};

/**
 * Like `getImport`, but returns ALL matching imports instead of just the first.
 *
 * Useful when the same specifier is imported multiple times - e.g. once via ESM
 * and once via CJS in a mixed codebase, or when scanning for every aliased
 * re-export. Namespace imports are only returned when no typed matches exist,
 * mirroring the fallback behaviour of `getImport`, but unlike `getImport` all
 * namespace imports are returned rather than just the first.
 *
 * @template T extends Language
 * @param program - The program node to search within.
 * @param options - Import lookup options. Use `{ type: "default", from }` for default/var forms, or `{ type: "named", name, from }` for a specific named specifier.
 * @returns An array of resolved imports (may be empty).
 */
export const getAllImports = <T extends Language>(
  program: SgNode<T, "program">,
  options: GetImportOptions,
): ResolvedImport<T>[] => {
  const matches = findRawImportMatches(program, options.from);
  if (matches.length === 0) return [];

  const results = matches.reduce<ResolvedImport<T>[]>((acc, match) => {
    const result = matchToImportResult<T>(match, options);
    if (result) acc.push(result);
    return acc;
  }, []);

  if (results.length === 0) {
    return matchToNamespaceImports<T>(matches);
  }

  return results;
};

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
  options: AddImportOptions,
): Edit | null {
  const moduleType = options.type === "namespace" ? "esm" : (options.moduleType ?? "esm");

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
            const insertPos = namedImports.range().start.index + closingBraceIdx;
            const specifierStr = newSpecifiers.map(formatSpecifier).join(", ");
            // Check if there are existing specifiers (need comma)
            const hasExistingSpecifiers =
              namedImportsText.slice(1, closingBraceIdx).trim().length > 0;
            const insertText = hasExistingSpecifiers ? `, ${specifierStr}` : ` ${specifierStr} `;

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
  const allImports = findAllImportStatements(program).sort(
    (a, b) => a.range().start.index - b.range().start.index,
  );
  let insertPos = 0;
  let prefix = "";

  const lastImport = allImports[allImports.length - 1];
  if (lastImport) {
    const programText = program.text();
    const importEnd = lastImport.range().end.index;
    insertPos = getStatementRangeWithNewline(lastImport, programText).end;

    if (insertPos === importEnd) {
      prefix = "\n";
    }
  }

  // Generate import text
  const importText =
    moduleType === "esm" ? generateEsmImport(options) : generateCjsRequire(options);

  return {
    startPos: insertPos,
    endPos: insertPos,
    insertedText: prefix + importText,
  };
}

/**
 * First argument to `require(...)`, after stripping redundant parentheses.
 * Does not recurse into nested calls — only literal module specifiers match.
 */
function unwrapParentheses(node: SgNode<TS>): SgNode<TS> {
  let current = node;
  while (current.kind() === "parenthesized_expression") {
    const inner = current.child(1);
    if (!inner) break;
    current = inner as SgNode<TS>;
  }
  return current;
}

function requireCallFirstArgIsLiteralSpecifier(
  requireCall: SgNode<TS>,
  packageName: string,
): boolean {
  const args = requireCall.field("arguments");
  if (!args) return false;
  let firstArg: SgNode<TS> | null = null;
  for (const child of args.children()) {
    const k = child.kind();
    if (k === "(" || k === ")" || k === ",") continue;
    firstArg = child as SgNode<TS>;
    break;
  }
  if (!firstArg) return false;
  const arg = unwrapParentheses(firstArg);
  if (arg.kind() !== "string") {
    return false;
  }
  const frag = arg.find({
    rule: {
      kind: "string_fragment",
      regex: stringToExactRegexString(packageName),
    },
  });
  return frag != null;
}

/**
 * `require('pkg');` as a standalone expression statement (e.g. polyfill registration).
 */
function removeBareRequireSideEffectEdit(
  program: SgNode<TS, "program">,
  packageName: string,
): Edit | null {
  const programText = program.text();
  for (const stmt of program.findAll({
    rule: { kind: "expression_statement" },
  })) {
    const expr = stmt.child(0);
    if (!expr || expr.kind() !== "call_expression") {
      continue;
    }
    const fn = expr.field("function");
    if (fn?.text() !== "require") {
      continue;
    }
    if (!requireCallFirstArgIsLiteralSpecifier(expr as SgNode<TS>, packageName)) {
      continue;
    }
    const { start, end } = getStatementRangeWithNewline(
      stmt as unknown as SgNode<Language>,
      programText,
    );
    return { startPos: start, endPos: end, insertedText: "" };
  }
  return null;
}

/**
 * Side-effect only: `import 'pkg'` / `import "pkg"` (no binding clause in grammar).
 * The module id is matched only on the statement’s `source` field (not other strings, e.g. import attributes).
 */
function removeSideEffectImportStatementEdit(
  program: SgNode<TS, "program">,
  packageName: string,
): Edit | null {
  const programText = program.text();
  for (const stmt of program.findAll({
    rule: { kind: "import_statement" },
  })) {
    const sourceNode = stmt.field("source");
    if (!sourceNode) {
      continue;
    }
    const frag = sourceNode.find({
      rule: {
        kind: "string_fragment",
        regex: stringToExactRegexString(packageName),
      },
    });
    if (!frag) {
      continue;
    }
    const hasBinding = stmt.find({
      rule: { kind: "import_clause" },
    });
    if (hasBinding) {
      continue;
    }
    const { start, end } = getStatementRangeWithNewline(
      stmt as unknown as SgNode<Language>,
      programText,
    );
    return { startPos: start, endPos: end, insertedText: "" };
  }
  return null;
}

/**
 * Remove an import from the program. Smart behavior:
 * - Default/namespace: Removes entire import statement
 * - Named (multiple specifiers exist): Removes only the specified specifiers
 * - Named (removing last specifiers): Removes entire import statement
 * - Default + `var` + `require()`: removed (same as `const`/`let`)
 * - Default + `removeSideEffectForms`: also removes bare `require('m')` and `import 'm'` when there is no binding import
 *
 * Note: removeImport returns null for multi-declarator CJS declarations
 * (e.g. `const foo = require('mod'), x = 1`). These are not tracked by
 * findAllImportStatements to prevent accidental deletion of unrelated bindings.
 *
 * @returns Edit to apply, or null if import not found
 */
export function removeImport<T extends Language>(
  program: SgNode<T, "program">,
  options: RemoveImportOptions,
): Edit | null {
  const programText = program.text();
  const allStatements = findAllImportStatements(program);

  if (options.type === "default") {
    const stripSideEffects = options.removeSideEffectForms === true;
    const existing = getImport(program, { type: "default", from: options.from });

    if (existing?.isNamespace) {
      return null;
    }

    if (existing) {
      const statement =
        allStatements.find((stmt) => {
          const tsStmt = stmt as unknown as SgNode<TS>;
          const matchesAlias = tsStmt.has({
            rule: {
              any: [{ kind: "identifier", regex: stringToExactRegexString(existing.alias) }],
            },
          });
          const matchesSource = tsStmt.has({
            rule: {
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
          });
          return matchesAlias && matchesSource;
        }) ?? null;

      if (!statement) return null;
      const { start, end } = getStatementRangeWithNewline(statement, programText);
      return { startPos: start, endPos: end, insertedText: "" };
    }

    if (stripSideEffects) {
      const tsProgram = program as unknown as SgNode<TS, "program">;
      return (
        removeBareRequireSideEffectEdit(tsProgram, options.from) ??
        removeSideEffectImportStatementEdit(tsProgram, options.from)
      );
    }

    return null;
  }

  if (options.type === "namespace") {
    const existing = getImport(program, { type: "default", from: options.from });
    if (!existing || !existing.isNamespace) return null;

    const statement =
      allStatements.find((stmt) => {
        const tsStmt = stmt as unknown as SgNode<TS>;
        return tsStmt.has({
          rule: {
            kind: "namespace_import",
            has: { kind: "identifier", regex: stringToExactRegexString(existing.alias) },
          },
        });
      }) ?? null;

    if (!statement) return null;
    const { start, end } = getStatementRangeWithNewline(statement, programText);
    return { startPos: start, endPos: end, insertedText: "" };
  }

  if (options.type === "named") {
    for (const specName of options.specifiers) {
      const found = findImportStatementForSpecifier(program, specName, options.from);
      if (!found) continue;

      const specifierCount = countSpecifiersInStatement(found.statement);

      // If this is the last specifier, remove the entire statement
      if (specifierCount <= options.specifiers.length) {
        const { start, end } = getStatementRangeWithNewline(found.statement, programText);
        return { startPos: start, endPos: end, insertedText: "" };
      }

      // Otherwise, just remove this specifier
      const { start, end } = getSpecifierRangeWithSeparator(found.specifier, programText);
      return { startPos: start, endPos: end, insertedText: "" };
    }

    return null;
  }

  return null;
}

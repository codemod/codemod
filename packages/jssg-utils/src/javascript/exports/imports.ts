import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { Rule, SgNode } from "@codemod.com/jssg-types/main";
import { stringToExactRegexString } from "../../utils";
import { getFileStyle } from "./style";

export { stringToExactRegexString };

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
  /** True for `import type` and for an inline `type` modifier on this binding. */
  isTypeOnly: boolean;
  moduleType: "esm" | "cjs"; // "esm" for import statements (static or dynamic), "cjs" for require() calls
  node: SgNode<T, "identifier">;
} | null;

type Language = JS | TS | TSX;

type ResolvedImport<T extends Language> = NonNullable<GetImportResult<T>>;

// ============================================================================
// Import Manipulation Types
// ============================================================================

type ImportSpecifier = { name: string; alias?: string; isTypeOnly?: boolean };

type AddImportOptions =
  | { type: "default"; name: string; from: string; moduleType?: "esm" | "cjs" }
  | { type: "namespace"; name: string; from: string }
  | {
      type: "named";
      specifiers: ImportSpecifier[];
      from: string;
      moduleType?: "esm" | "cjs";
    };

type RemoveNamedSpecifier = string | { name: string; alias?: string };

type RemoveImportOptions =
  | {
      type: "default";
      from: string;
      /**
       * Local binding to remove when the module has more than one default import.
       * Omit to remove the first default binding.
       */
      alias?: string;
      /**
       * When `getImport` finds no binding, also remove side-effect-only lines:
       * `import 'module'` and `require('module');`. Default false.
       */
      removeSideEffectForms?: boolean;
    }
  | {
      type: "namespace";
      from: string;
      /** Local binding to remove when the module has more than one namespace import. */
      alias?: string;
    }
  | { type: "named"; specifiers: RemoveNamedSpecifier[]; from: string };

type UpdateImportOptions = {
  type: "named";
  from: string;
  specifiers: Array<{
    name: string;
    to: string;
    /** Replaces the local binding. Omit to keep the current alias. */
    alias?: string;
    /**
     * Current local name to rename. Use this when the same export is imported
     * more than once (`foo` and `foo as bar`, or two statements). Omit to
     * rename every binding of `name`.
     */
    local?: string;
  }>;
};

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

  const callbackObjectPattern = {
    kind: "object_pattern",
    any: [
      { inside: { kind: "formal_parameters" } },
      { inside: { field: "pattern", inside: { kind: "formal_parameters" } } },
      {
        inside: {
          kind: "assignment_pattern",
          field: "left",
          inside: { kind: "formal_parameters" },
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
        {
          all: [
            {
              any: [
                // - dynamic with callback: import("mod").then((mod) => ...) or import("mod").then(function(mod) { ... })
                {
                  kind: "identifier",
                  pattern: "$NAMESPACE_ALIAS",
                  inside: {
                    kind: "formal_parameters",
                  },
                },
                // TypeScript parameter wrappers expose their binding in the pattern field.
                {
                  kind: "identifier",
                  pattern: "$NAMESPACE_ALIAS",
                  inside: {
                    field: "pattern",
                    inside: {
                      kind: "formal_parameters",
                    },
                  },
                },
                // - dynamic with callback: import("mod").then(mod => ...)
                {
                  kind: "identifier",
                  pattern: "$NAMESPACE_ALIAS",
                  inside: {
                    kind: "arrow_function",
                    field: "parameter",
                  },
                },
                // - dynamic with destructured callback: import("mod").then(({test}) => ...)
                {
                  kind: "shorthand_property_identifier_pattern",
                  pattern: "$ORIGINAL",
                  inside: callbackObjectPattern,
                },
                // - dynamic with destructured + alias callback: import("mod").then(({test: alias}) => ...)
                {
                  all: [
                    { kind: "pair_pattern", inside: callbackObjectPattern },
                    { has: { field: "key", kind: "property_identifier", pattern: "$ORIGINAL" } },
                    {
                      not: { has: { field: "key", kind: "property_identifier", regex: "default" } },
                    },
                    { has: { field: "value", kind: "identifier", pattern: "$ALIAS" } },
                  ],
                },
                // - dynamic with destructured callback using default: import("mod").then(({default: test}) => ...)
                {
                  all: [
                    { kind: "pair_pattern", inside: callbackObjectPattern },
                    { has: { field: "key", kind: "property_identifier", regex: "default" } },
                    { has: { field: "value", kind: "identifier", pattern: "$VAR_NAME" } },
                  ],
                },
              ],
            },
            {
              inside: {
                stopBy: {
                  any: [
                    { kind: "arrow_function" },
                    { kind: "function_expression" },
                    { kind: "function_declaration" },
                    { kind: "generator_function" },
                    { kind: "generator_function_declaration" },
                    { kind: "method_definition" },
                  ],
                },
                any: [{ kind: "arrow_function" }, { kind: "function_expression" }],
                inside: {
                  kind: "arguments",
                  inside: {
                    kind: "call_expression",
                    has: {
                      field: "function",
                      kind: "member_expression",
                      all: [
                        {
                          has: {
                            field: "property",
                            kind: "property_identifier",
                            regex: "^then$",
                          },
                        },
                        {
                          has: {
                            field: "object",
                            kind: "call_expression",
                            has: {
                              field: "arguments",
                              kind: "arguments",
                              has: {
                                kind: "string",
                                has: {
                                  kind: "string_fragment",
                                  pattern: "$SOURCE",
                                },
                              },
                            },
                          },
                        },
                      ],
                    },
                  },
                },
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
 * True when `node` is a type-only binding: `import type`, `import { type X }`,
 * or the default of `import type Foo`. A value import of the same specifier is
 * a different binding. CommonJS has no type-only form.
 */
function bindingIsTypeOnly(node: SgNode<TS>): boolean {
  if (importSpecifierIsTypeOnly(node)) return true;
  const statement =
    node.kind() === "import_statement"
      ? node
      : ((node.ancestors().find((ancestor) => ancestor.kind() === "import_statement") as
          | SgNode<TS>
          | undefined) ?? null);
  return statement ? statementIsImportType(statement) : false;
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
      isTypeOnly: bindingIsTypeOnly(nameNode),
      moduleType: getModuleType(match),
      node: nameNode as unknown as SgNode<T, "identifier">,
    };
  }

  if (options.type === "named") {
    const original = match.getMatch("ORIGINAL");
    if (original?.text() !== options.name) return null;
    const alias = match.getMatch("ALIAS");
    const node = (alias ?? original)!;
    return {
      alias: alias?.text() ?? original?.text() ?? "",
      isNamespace: false,
      isTypeOnly: bindingIsTypeOnly(node),
      moduleType: getModuleType(match),
      node: node as unknown as SgNode<T, "identifier">,
    };
  }

  return null;
}

/**
 * Extract ALL namespace import results from a list of raw matches.
 */
function namespaceImportFromStatement<T extends Language>(
  statement: SgNode<TS>,
): ResolvedImport<T> | null {
  const namespace = statement.find({ rule: { kind: "namespace_import" } });
  const identifier = namespace?.children().find((child) => child.kind() === "identifier");
  if (!identifier) return null;
  return {
    alias: identifier.text(),
    isNamespace: true,
    isTypeOnly: statementIsImportType(statement),
    moduleType: "esm",
    node: identifier as unknown as SgNode<T, "identifier">,
  };
}

function matchToNamespaceImports<T extends Language>(matches: SgNode<TS>[]): ResolvedImport<T>[] {
  const found: ResolvedImport<T>[] = [];
  const seen = new Set<number>();
  const add = (item: ResolvedImport<T> | null) => {
    if (!item) return;
    const index = item.node.range().start.index;
    if (seen.has(index)) return;
    seen.add(index);
    found.push(item);
  };

  for (const match of matches) {
    const captured = match.getMatch("NAMESPACE_ALIAS");
    if (captured) {
      add({
        alias: captured.text(),
        isNamespace: true,
        isTypeOnly: bindingIsTypeOnly(captured),
        moduleType: "esm",
        node: captured as unknown as SgNode<T, "identifier">,
      });
    }

    const statement =
      match.kind() === "import_statement"
        ? match
        : ((match.ancestors().find((ancestor) => ancestor.kind() === "import_statement") as
            | SgNode<TS>
            | undefined) ?? null);
    if (statement) add(namespaceImportFromStatement(statement));
  }

  return found;
}

function dedupeResolvedImports<T extends Language>(
  items: ResolvedImport<T>[],
): ResolvedImport<T>[] {
  const seen = new Set<number>();
  const unique: ResolvedImport<T>[] = [];
  for (const item of items) {
    const index = item.node.range().start.index;
    if (seen.has(index)) continue;
    seen.add(index);
    unique.push(item);
  }
  return unique;
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
          kind: "call_expression",
          has: {
            field: "function",
            kind: "identifier",
            regex: "^require$",
          },
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

  // `var foo = await import(...)`
  // `import(...)`
  // `import(...).then((...) => void)`
  const dynamicImports = tsProgram.findAll({
    rule: {
      any: [
        {
          kind: "variable_declaration",
          has: {
            kind: "variable_declarator",
            has: {
              field: "value",
              kind: "await_expression",
              has: {
                kind: "call_expression",
                has: {
                  field: "function",
                  regex: "^import$",
                },
              },
            },
          },
          not: {
            has: {
              kind: "variable_declarator",
              nthChild: 2,
            },
          },
        },
        {
          kind: "expression_statement",
          has: {
            kind: "call_expression",
            has: {
              stopBy: "end",
              field: "function",
              regex: "^import$",
            },
          },
        },
      ],
    },
  });

  return [
    ...esmImports,
    ...cjsImports,
    ...cjsVarImports,
    ...dynamicImports,
  ] as unknown as SgNode<T>[];
}

/**
 * Every ESM import of `source`, in source order.
 * Callers that merge a value specifier must skip an earlier `import type`.
 */
function findEsmImportsFrom<T extends Language>(
  program: SgNode<T, "program">,
  source: string,
): Array<SgNode<T, "import_statement">> {
  const tsProgram = program as unknown as SgNode<TS, "program">;
  const imports = tsProgram.findAll({
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

  return imports
    .slice()
    .sort(
      (left, right) => left.range().start.index - right.range().start.index,
    ) as unknown as Array<SgNode<T, "import_statement">>;
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
function formatSpecifier(
  spec: ImportSpecifier,
  kind: "named_imports" | "object_pattern" = "named_imports",
): string {
  const typePrefix = spec.isTypeOnly && kind === "named_imports" ? "type " : "";
  if (spec.alias && spec.alias !== spec.name) {
    if (kind === "named_imports") return `${typePrefix}${spec.name} as ${spec.alias}`;
    if (kind === "object_pattern") return `${spec.name}: ${spec.alias}`;
  }
  return `${typePrefix}${spec.name}`;
}

interface StatementStyle {
  quote: string;
  semicolon: string;
}

function quoteSource(source: string, quote: string): string {
  return `${quote}${source}${quote}`;
}

/**
 * Generate ESM import statement text.
 * Type-only named specifiers use an inline `type` modifier, not `import type`,
 * so a later value specifier can still join the same statement.
 */
function generateEsmImport(options: AddImportOptions, style: StatementStyle): string {
  const source = quoteSource(options.from, style.quote);
  const semicolon = style.semicolon;

  if (options.type === "default") {
    return `import ${options.name} from ${source}${semicolon}\n`;
  }

  if (options.type === "namespace") {
    return `import * as ${options.name} from ${source}${semicolon}\n`;
  }

  const specifierStr = options.specifiers.map((spec) => formatSpecifier(spec)).join(", ");
  return `import { ${specifierStr} } from ${source}${semicolon}\n`;
}

/**
 * Generate CJS require statement text.
 */
function generateCjsRequire(options: AddImportOptions, style: StatementStyle): string {
  const source = quoteSource(options.from, style.quote);
  const semicolon = style.semicolon;

  if (options.type === "default") {
    return `const ${options.name} = require(${source})${semicolon}\n`;
  }

  if (options.type === "namespace") {
    // Namespace-like for CJS
    return `const ${options.name} = require(${source})${semicolon}\n`;
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
  return `const { ${specifierStr} } = require(${source})${semicolon}\n`;
}

/**
 * Parts of an ESM `import_clause`. An import_statement can pair a default
 * binding with either a `named_imports` or a `namespace_import`, and removing
 * one half of such a mixed clause must leave the other half intact.
 */
type ImportClauseParts = {
  defaultIdent: SgNode<TS> | null;
  namespaceImport: SgNode<TS> | null;
  namedImports: SgNode<TS> | null;
};

function analyzeImportClause<T extends Language>(importStmt: SgNode<T>): ImportClauseParts {
  const tsStmt = importStmt as unknown as SgNode<TS>;
  const parts: ImportClauseParts = {
    defaultIdent: null,
    namespaceImport: null,
    namedImports: null,
  };
  const clause = tsStmt.find({ rule: { kind: "import_clause" } });
  if (!clause) return parts;

  for (const child of clause.children()) {
    const k = child.kind();
    if (k === "identifier" && !parts.defaultIdent) {
      parts.defaultIdent = child as SgNode<TS>;
    } else if (k === "namespace_import") {
      parts.namespaceImport = child as SgNode<TS>;
    } else if (k === "named_imports") {
      parts.namedImports = child as SgNode<TS>;
    }
  }
  return parts;
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

function hasTrailingComma<T extends Language>(nodes: SgNode<T>[]) {
  const closeBraceIdx = nodes.findLastIndex((n) => n.is("}"));
  // check if the node before the closing brace `}` is a comma, ignoring comments.
  for (let i = closeBraceIdx - 1; i > 0; i--) {
    const node = nodes[i];

    if (node?.is("comment")) {
      continue;
    }

    return node?.is(",");
  }

  return false;
}

type ClauseItem = { kind: "comment" | "specifier"; text: string };

/**
 * Rebuild a `named_imports` / `object_pattern` clause from an ordered list of
 * items (specifiers and comments), preserving the original single/multi-line
 * layout and trailing comma. Returns a single edit that replaces the whole
 * clause node, so callers never produce overlapping edits.
 */
function serializeSpecifierClause<T extends Language>(
  parentNode: SgNode<T>,
  items: ClauseItem[],
): Edit {
  const isMultiline = parentNode.range().start.line !== parentNode.range().end.line;
  const lastItem = items[items.length - 1];
  const trailingComma =
    hasTrailingComma(parentNode.children()) && lastItem?.kind === "specifier" ? "," : "";

  let body = "";
  for (let i = 0; i < items.length; i++) {
    const item = items[i]!;
    const next = items[i + 1];

    if (!next) {
      body += item.text;
      continue;
    }

    if (!isMultiline) {
      body += item.text;
      if (item.kind === "comment") {
        body += " ";
      } else {
        body += ", ";
      }
      continue;
    }

    if (item.kind === "comment") {
      body += item.text + "\n  ";
      continue;
    }

    body += item.text + (next.kind === "comment" ? "," : ",\n  ");
  }

  body += trailingComma;

  return isMultiline ? parentNode.replace(`{\n  ${body}\n}`) : parentNode.replace(`{ ${body} }`);
}

/**
 * Read the existing children of a specifier clause as items, keeping comments
 * in place.
 */
function readClauseItems<T extends Language>(
  parentNode: SgNode<T>,
): Array<ClauseItem & { node: SgNode<T> }> {
  return parentNode
    .children()
    .filter((n) => n.isNamed() || n.is("comment"))
    .map((n) => ({ kind: n.is("comment") ? "comment" : "specifier", text: n.text(), node: n }));
}

function mergeSpecifiers<T extends Language>(
  parentNode: SgNode<T>,
  newSpecifiers: ImportSpecifier[],
) {
  const kind = parentNode.kind() === "object_pattern" ? "object_pattern" : "named_imports";

  const items: ClauseItem[] = [
    ...readClauseItems(parentNode),
    ...newSpecifiers.map((spec) => ({
      kind: "specifier" as const,
      text: formatSpecifier(spec, kind),
    })),
  ];
  return serializeSpecifierClause(parentNode, items);
}

/**
 * Resolve the imported name and local alias of a specifier node:
 * - `import_specifier`: `foo` / `foo as f`
 * - `pair_pattern`: `foo: f`
 * - `shorthand_property_identifier_pattern`: `foo`
 */
function readSpecifier<T extends Language>(
  node: SgNode<T>,
): { name: string; alias?: string } | null {
  const tsNode = node as unknown as SgNode<TS>;
  const kind = tsNode.kind();

  if (kind === "import_specifier") {
    const name = tsNode.field("name")?.text();
    if (!name) return null;
    const alias = tsNode.field("alias")?.text();
    return alias ? { name, alias } : { name };
  }

  if (kind === "pair_pattern") {
    const name = tsNode.field("key")?.text();
    const alias = tsNode.field("value")?.text();
    if (!name) return null;
    return alias ? { name, alias } : { name };
  }

  if (kind === "shorthand_property_identifier_pattern") {
    return { name: tsNode.text() };
  }

  return null;
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
 * re-export. A `{ type: "default" }` query includes namespace imports even when
 * a default import exists. A named query still uses namespace imports only as
 * a fallback when that export is not imported by name. `isTypeOnly` marks
 * `import type` and inline `type` bindings.
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

  const namespaces = matchToNamespaceImports<T>(matches);
  if (options.type === "default") {
    return dedupeResolvedImports([...results, ...namespaces]);
  }

  if (results.length === 0) {
    return namespaces;
  }

  return results;
};

export interface ListedSpecifier<T extends Language> {
  importedName: string;
  localName: string;
  /** True for `import type` and for an inline `type` modifier. */
  isTypeOnly: boolean;
  node: SgNode<T>;
}

export interface ListedImport<T extends Language> {
  node: SgNode<T>;
  source: string;
  quote: string;
  hasSemicolon: boolean;
  moduleType: "esm" | "cjs";
  /** True only for a statement-level `import type`, not an inline `type` modifier. */
  isTypeOnly: boolean;
  defaultName: string | null;
  namespaceName: string | null;
  specifiers: Array<ListedSpecifier<T>>;
}

export type ListImportsOptions = {
  /** Exact module specifier, or a list of them. `"mod"` does not match `"mod/extra"`. */
  from?: string | readonly string[];
};

function stringLiteralParts(stringNode: SgNode<TS>): { source: string; quote: string } | null {
  const fragments = stringNode.findAll({ rule: { kind: "string_fragment" } });
  if (fragments.length === 0) return null;
  const quote = stringNode.text().charAt(0);
  return {
    source: fragments.map((fragment) => fragment.text()).join(""),
    quote: quote === "'" || quote === '"' || quote === "`" ? quote : "'",
  };
}

function statementHasSemicolon(statement: SgNode<TS>): boolean {
  return statement.text().trimEnd().endsWith(";");
}

function literalSpecifierOfCall(call: SgNode<TS>): { source: string; quote: string } | null {
  const args = call.field("arguments");
  if (!args || args.kind() !== "arguments") return null;

  let first: SgNode<TS> | null = null;
  for (const child of args.children()) {
    const kind = child.kind();
    if (kind === "(" || kind === ")" || kind === ",") continue;
    first = child as SgNode<TS>;
    break;
  }
  if (!first) return null;

  const arg = unwrapParentheses(first);
  if (arg.kind() !== "string") return null;
  return stringLiteralParts(arg);
}

function moduleTypeOfCall(call: SgNode<TS>): "esm" | "cjs" | null {
  const fn = call.field("function");
  if (!fn) return null;
  if (fn.text() === "require") return "cjs";
  if (fn.text() === "import" || fn.kind() === "import") return "esm";
  return null;
}

function specifiersFromObjectPattern(pattern: SgNode<TS>): Array<ListedSpecifier<TS>> {
  const specifiers: Array<ListedSpecifier<TS>> = [];
  for (const child of pattern.children()) {
    if (
      child.kind() !== "shorthand_property_identifier_pattern" &&
      child.kind() !== "pair_pattern"
    ) {
      continue;
    }
    const read = readSpecifier(child);
    if (!read) continue;
    specifiers.push({
      importedName: read.name,
      localName: read.alias ?? read.name,
      isTypeOnly: false,
      node: child,
    });
  }
  return specifiers;
}

function describeEsmImport(statement: SgNode<TS>): ListedImport<TS> | null {
  const sourceNode = statement.field("source");
  if (!sourceNode || sourceNode.kind() !== "string") return null;
  const literal = stringLiteralParts(sourceNode as SgNode<TS>);
  if (!literal) return null;

  const isTypeOnly = statement.children().some((child) => child.kind() === "type");
  let defaultName: string | null = null;
  let namespaceName: string | null = null;
  const clause = statement.find({ rule: { kind: "import_clause" } });
  if (clause) {
    for (const child of clause.children()) {
      if (child.kind() === "identifier" && defaultName === null) {
        defaultName = child.text();
      } else if (child.kind() === "namespace_import") {
        const identifier = child.children().find((inner) => inner.kind() === "identifier");
        namespaceName = identifier?.text() ?? null;
      }
    }
  }

  const specifiers: Array<ListedSpecifier<TS>> = [];
  const named = findNamedImports(statement);
  if (named) {
    for (const specifier of named.findAll({ rule: { kind: "import_specifier" } })) {
      const read = readSpecifier(specifier);
      if (!read) continue;
      const inlineType = specifier.children().some((child) => child.kind() === "type");
      specifiers.push({
        importedName: read.name,
        localName: read.alias ?? read.name,
        isTypeOnly: isTypeOnly || inlineType,
        node: specifier,
      });
    }
  }

  return {
    node: statement,
    source: literal.source,
    quote: literal.quote,
    hasSemicolon: statementHasSemicolon(statement),
    moduleType: "esm",
    isTypeOnly,
    defaultName,
    namespaceName,
    specifiers,
  };
}

function asImportCall(value: SgNode<TS>): { call: SgNode<TS>; moduleType: "esm" | "cjs" } | null {
  let current = value;
  if (current.kind() === "await_expression") {
    const inner = current.children().find((child) => child.kind() === "call_expression");
    if (!inner) return null;
    current = inner as SgNode<TS>;
  }
  if (current.kind() !== "call_expression") return null;
  const moduleType = moduleTypeOfCall(current);
  if (!moduleType) return null;
  return { call: current, moduleType };
}

function describeDeclaration(statement: SgNode<TS>): ListedImport<TS> | null {
  const declarator = statement.find({ rule: { kind: "variable_declarator" } }) as SgNode<TS> | null;
  if (!declarator) return null;
  const name = declarator.field("name");
  const value = declarator.field("value");
  if (!name || !value) return null;

  const imported = asImportCall(value as SgNode<TS>);
  if (!imported) return null;
  const literal = literalSpecifierOfCall(imported.call);
  if (!literal) return null;

  let defaultName: string | null = null;
  let specifiers: Array<ListedSpecifier<TS>> = [];
  if (name.kind() === "identifier") {
    defaultName = name.text();
  } else if (name.kind() === "object_pattern") {
    specifiers = specifiersFromObjectPattern(name as SgNode<TS>);
  }

  return {
    node: statement,
    source: literal.source,
    quote: literal.quote,
    hasSemicolon: statementHasSemicolon(statement),
    moduleType: imported.moduleType,
    isTypeOnly: false,
    defaultName,
    namespaceName: null,
    specifiers,
  };
}

/** `({ foo } = {})` and `required_parameter` wrappers expose the pattern on the left. */
function unwrapBindingPattern(node: SgNode<TS> | null): SgNode<TS> | null {
  if (!node) return null;
  let current = node;
  if (current.kind() === "required_parameter") {
    const named = current.children().find((inner) => inner.isNamed());
    current = (named as SgNode<TS> | undefined) ?? current;
  }
  if (current.kind() === "assignment_pattern") {
    const left = current.field("left") ?? current.child(0);
    if (left) current = left as SgNode<TS>;
  }
  return current;
}

function describeOneBinding(node: SgNode<TS> | null): {
  namespaceName: string | null;
  specifiers: Array<ListedSpecifier<TS>>;
} {
  const binding = unwrapBindingPattern(node);
  if (!binding) return { namespaceName: null, specifiers: [] };
  if (binding.kind() === "identifier") {
    return { namespaceName: binding.text(), specifiers: [] };
  }
  if (binding.kind() === "object_pattern") {
    return { namespaceName: null, specifiers: specifiersFromObjectPattern(binding) };
  }
  return { namespaceName: null, specifiers: [] };
}

function describeCallbackBindings(fn: SgNode<TS>): {
  namespaceName: string | null;
  specifiers: Array<ListedSpecifier<TS>>;
} {
  const direct = fn.field("parameter");
  const params = fn.field("parameters");
  if (direct && !params) return describeOneBinding(direct as SgNode<TS>);
  if (!params) return { namespaceName: null, specifiers: [] };

  let namespaceName: string | null = null;
  const specifiers: Array<ListedSpecifier<TS>> = [];
  for (const child of params.children()) {
    const described = describeOneBinding(child as SgNode<TS>);
    if (described.namespaceName) namespaceName = described.namespaceName;
    specifiers.push(...described.specifiers);
  }
  return { namespaceName, specifiers };
}

/**
 * The callback of `import(...).then(...)`, not a `.catch` / `.finally` callback
 * or an unrelated function in the same statement.
 */
function thenCallbackOfImport(importCall: SgNode<TS>): SgNode<TS> | null {
  const ancestors = importCall.ancestors() as SgNode<TS>[];
  let cursor = 0;
  let receiver: SgNode<TS> = importCall;
  while (ancestors[cursor]?.kind() === "parenthesized_expression") {
    receiver = ancestors[cursor]!;
    cursor += 1;
  }

  const member = ancestors[cursor];
  if (!member || member.kind() !== "member_expression") return null;
  const object = member.field("object");
  const property = member.field("property");
  if (!object || object.range().start.index !== receiver.range().start.index) return null;
  if (property?.text() !== "then") return null;

  const thenCall = ancestors[cursor + 1];
  if (!thenCall || thenCall.kind() !== "call_expression") return null;
  const called = thenCall.field("function");
  if (!called || called.range().start.index !== member.range().start.index) return null;

  const args = thenCall.field("arguments");
  if (!args) return null;
  for (const child of args.children()) {
    const kind = child.kind();
    if (kind === "(" || kind === ")" || kind === ",") continue;
    if (kind === "arrow_function" || kind === "function_expression") {
      return child as SgNode<TS>;
    }
    return null;
  }
  return null;
}

function describeDynamicExpression(statement: SgNode<TS>): ListedImport<TS> | null {
  const importCall = statement.find({
    rule: {
      kind: "call_expression",
      has: { field: "function", regex: "^import$" },
    },
  }) as SgNode<TS> | null;
  if (!importCall) return null;
  const literal = literalSpecifierOfCall(importCall);
  if (!literal) return null;

  const callback = thenCallbackOfImport(importCall);
  const bindings = callback
    ? describeCallbackBindings(callback)
    : { namespaceName: null, specifiers: [] as Array<ListedSpecifier<TS>> };

  return {
    node: statement,
    source: literal.source,
    quote: literal.quote,
    hasSemicolon: statementHasSemicolon(statement),
    moduleType: "esm",
    isTypeOnly: false,
    defaultName: null,
    namespaceName: bindings.namespaceName,
    specifiers: bindings.specifiers,
  };
}

function describeImportStatement(statement: SgNode<TS>): ListedImport<TS> | null {
  const kind = statement.kind();
  if (kind === "import_statement") return describeEsmImport(statement);
  if (kind === "lexical_declaration" || kind === "variable_declaration") {
    return describeDeclaration(statement);
  }
  if (kind === "expression_statement") return describeDynamicExpression(statement);
  return null;
}

/**
 * List import statements and the require/import() declarations this module
 * already knows how to edit.
 *
 * Unlike `getAllImports`, this does not ask for a specifier name. Pass `from`
 * to keep statements whose module specifier is exactly one of those strings.
 *
 * Multi-declarator CommonJS (`const a = require("mod"), b = 1`) is omitted,
 * matching `removeImport`, so a later deletion cannot drop `b`.
 * `const x = import("mod")` without `await` is omitted. `import("mod").then(...)`
 * is included. A `.catch` or `.finally` callback is not an imported binding.
 */
export function listImports<T extends Language>(
  program: SgNode<T, "program">,
  options?: ListImportsOptions,
): Array<ListedImport<T>> {
  const from = options?.from;
  const allowed = from === undefined ? null : new Set(typeof from === "string" ? [from] : from);
  const listed: Array<ListedImport<T>> = [];

  for (const statement of findAllImportStatements(program)) {
    const described = describeImportStatement(statement as unknown as SgNode<TS>);
    if (!described) continue;
    if (allowed && !allowed.has(described.source)) continue;
    listed.push(described as unknown as ListedImport<T>);
  }

  listed.sort((left, right) => left.node.range().start.index - right.node.range().start.index);
  return listed;
}

function statementIsImportType<T extends Language>(importStmt: SgNode<T>): boolean {
  return importStmt.children().some((child) => child.kind() === "type");
}

function withoutInlineType(specifiers: ImportSpecifier[]): ImportSpecifier[] {
  return specifiers.map((spec) => ({ name: spec.name, alias: spec.alias }));
}

function bindingKey(importedName: string, localName: string): string {
  return `${importedName}\0${localName}`;
}

function requestedLocalName(spec: ImportSpecifier): string {
  return spec.alias ?? spec.name;
}

/**
 * A named specifier is already bound when the same export uses the same local
 * name. `import { foo as bar }` does not satisfy `{ name: "foo" }`. A value
 * binding satisfies a type request. A type-only binding does not satisfy a
 * value request. Namespace imports are not named bindings.
 */
function namedSpecifierExists<T extends Language>(
  program: SgNode<T, "program">,
  from: string,
  spec: ImportSpecifier,
): boolean {
  const local = requestedLocalName(spec);
  // getAllImports sees multi-declarator requires and defaulted callback
  // patterns that listImports omits so removal cannot drop a sibling binding.
  return getAllImports(program, { type: "named", name: spec.name, from }).some((existing) => {
    if (existing.isNamespace || existing.alias !== local) return false;
    if (!spec.isTypeOnly && existing.isTypeOnly) return false;
    return true;
  });
}

function isModuleLevelStatement(statement: SgNode<TS>): boolean {
  let parent = statement.parent();
  while (parent?.kind() === "export_statement") {
    parent = parent.parent();
  }
  return parent?.kind() === "program";
}

/**
 * True when `node` is the local name of `import type` or `import { type X }`.
 * A value import of the same specifier is a different binding.
 */
function importSpecifierIsTypeOnly(node: SgNode<TS>): boolean {
  const specifier =
    node.kind() === "import_specifier"
      ? node
      : ((node.ancestors().find((ancestor) => ancestor.kind() === "import_specifier") as
          | SgNode<TS>
          | undefined) ?? null);
  if (!specifier) return false;
  if (specifier.children().some((child) => child.kind() === "type")) return true;
  const statement = specifier
    .ancestors()
    .find((ancestor) => ancestor.kind() === "import_statement") as SgNode<TS> | undefined;
  return statement?.children().some((child) => child.kind() === "type") ?? false;
}

function dropStatementTypeKeyword(statement: SgNode<TS>): Edit | null {
  const typeToken = statement.children().find((child) => child.kind() === "type");
  const clause = statement.find({ rule: { kind: "import_clause" } });
  if (!typeToken || !clause) return null;
  return {
    startPos: typeToken.range().start.index,
    endPos: clause.range().start.index,
    insertedText: "",
  };
}

function dropInlineTypeKeyword(specifier: SgNode<TS>): Edit | null {
  const typeToken = specifier.children().find((child) => child.kind() === "type");
  const name = specifier.field("name");
  if (!typeToken || !name) return null;
  return {
    startPos: typeToken.range().start.index,
    endPos: name.range().start.index,
    insertedText: "",
  };
}

function renderPromotedImport(
  described: ListedImport<TS>,
  keys: Set<string>,
  extras: ImportSpecifier[],
): string {
  const specTexts = described.specifiers.map((spec) => {
    const promoted = keys.has(bindingKey(spec.importedName, spec.localName));
    const alias = spec.localName !== spec.importedName ? ` as ${spec.localName}` : "";
    const typePrefix = spec.isTypeOnly && !promoted ? "type " : "";
    return `${typePrefix}${spec.importedName}${alias}`;
  });
  for (const spec of extras) {
    specTexts.push(formatSpecifier(spec));
  }

  const source = `${described.quote}${described.source}${described.quote}`;
  const semi = described.hasSemicolon ? ";" : "";
  const named = `{ ${specTexts.join(", ")} }`;
  if (described.defaultName) {
    return `import ${described.defaultName}, ${named} from ${source}${semi}`;
  }
  return `import ${named} from ${source}${semi}`;
}

interface PromotionTarget {
  statement: SgNode<TS>;
  described: ListedImport<TS>;
  keys: Set<string>;
}

/**
 * Turn a type-only binding into a value binding when the local name matches.
 * `import { type Foo }` drops `type`. `import type { Foo, Bar }` becomes
 * `import { Foo, type Bar }`. A different local name is left for a new statement.
 */
function promotionEdit<T extends Language>(
  program: SgNode<T, "program">,
  from: string,
  pending: ImportSpecifier[],
): Edit | null {
  const targets = new Map<number, PromotionTarget>();
  for (const spec of pending) {
    if (spec.isTypeOnly) continue;
    const local = requestedLocalName(spec);
    const key = bindingKey(spec.name, local);
    for (const item of listImports(program, { from })) {
      if (item.moduleType !== "esm") continue;
      const match = item.specifiers.some(
        (existing) =>
          existing.isTypeOnly &&
          existing.importedName === spec.name &&
          existing.localName === local,
      );
      if (!match) continue;
      const id = item.node.range().start.index;
      const target = targets.get(id) ?? {
        statement: item.node as unknown as SgNode<TS>,
        described: item as unknown as ListedImport<TS>,
        keys: new Set<string>(),
      };
      target.keys.add(key);
      targets.set(id, target);
      break;
    }
  }

  const first = [...targets.values()].sort(
    (left, right) => left.statement.range().start.index - right.statement.range().start.index,
  )[0];
  if (!first) return null;

  const extras = pending.filter((spec) => {
    const key = bindingKey(spec.name, requestedLocalName(spec));
    for (const target of targets.values()) {
      if (target.keys.has(key)) return false;
    }
    return true;
  });

  const { statement, described, keys } = first;
  const allPromoted = described.specifiers.every((spec) =>
    keys.has(bindingKey(spec.importedName, spec.localName)),
  );

  if (described.isTypeOnly && allPromoted && extras.length === 0) {
    const dropped = dropStatementTypeKeyword(statement);
    if (dropped) return dropped;
  }

  if (described.isTypeOnly) {
    return statement.replace(renderPromotedImport(described, keys, extras));
  }

  const named = findNamedImports(statement);
  if (!named) {
    return statement.replace(renderPromotedImport(described, keys, extras));
  }

  if (extras.length === 0 && keys.size === 1) {
    const match = described.specifiers.find((spec) =>
      keys.has(bindingKey(spec.importedName, spec.localName)),
    );
    if (match) {
      const dropped = dropInlineTypeKeyword(match.node as unknown as SgNode<TS>);
      if (dropped) return dropped;
    }
  }

  const items: ClauseItem[] = readClauseItems(named).map((item) => {
    if (item.kind !== "specifier") return { kind: item.kind, text: item.text };
    const read = readSpecifier(item.node);
    if (!read) return { kind: item.kind, text: item.text };
    const local = read.alias ?? read.name;
    if (keys.has(bindingKey(read.name, local))) {
      return { kind: "specifier" as const, text: item.text.replace(/^type\s+/, "") };
    }
    return { kind: item.kind, text: item.text };
  });
  for (const spec of extras) {
    items.push({ kind: "specifier", text: formatSpecifier(spec) });
  }
  return serializeSpecifierClause(named, items);
}

/**
 * Merge named specifiers into an existing ESM import or dynamic import()
 * callback. Returns null when the caller should insert a new statement.
 *
 * A value specifier is not merged into `import type`: that statement cannot
 * later hold a value. An earlier `import type` does not hide a later value
 * import from the same module. Type-only specifiers are not merged into a
 * dynamic `import().then(({ ... }) => ...)` pattern, which has no `type` modifier.
 */
function mergeNamedIntoExisting<T extends Language>(
  program: SgNode<T, "program">,
  from: string,
  newSpecifiers: ImportSpecifier[],
): Edit | null {
  const hasValue = newSpecifiers.some((spec) => !spec.isTypeOnly);
  for (const existingImport of findEsmImportsFrom(program, from)) {
    if (statementIsImportType(existingImport) && hasValue) continue;

    const importType = statementIsImportType(existingImport);
    const specs = importType ? withoutInlineType(newSpecifiers) : newSpecifiers;
    const namedImports = findNamedImports(existingImport);
    if (namedImports) {
      return mergeSpecifiers(namedImports, specs);
    }

    // import foo from 'mod' -> import foo, { bar } from 'mod'
    // A namespace clause cannot also take named specifiers.
    const importClause = (existingImport as unknown as SgNode<TS>).find({
      rule: { kind: "import_clause" },
    });
    if (!importClause || importClause.find({ rule: { kind: "namespace_import" } })) {
      continue;
    }
    const specifierStr = specs.map((spec) => formatSpecifier(spec)).join(", ");
    const insertPos = importClause.range().end.index;
    return {
      startPos: insertPos,
      endPos: insertPos,
      insertedText: `, { ${specifierStr} }`,
    };
  }

  if (newSpecifiers.some((spec) => spec.isTypeOnly)) {
    return null;
  }

  const dynamicImportObjectPattern = (program as unknown as SgNode<TS, "program">).find({
    rule: {
      kind: "object_pattern",
      inside: {
        stopBy: "end",
        kind: "call_expression",
        has: {
          kind: "member_expression",
          has: {
            kind: "call_expression",
            all: [
              {
                has: {
                  field: "function",
                  kind: "import",
                },
              },
              {
                has: {
                  field: "arguments",
                  kind: "arguments",
                  has: {
                    kind: "string",
                    has: {
                      kind: "string_fragment",
                      regex: stringToExactRegexString(from),
                    },
                  },
                },
              },
            ],
          },
        },
      },
    },
  });

  if (dynamicImportObjectPattern) {
    return mergeSpecifiers(dynamicImportObjectPattern, newSpecifiers);
  }

  return null;
}

/**
 * Add an import to the program. Smart behavior:
 * - Skip if the import already exists
 * - Merge into existing import statement for named imports
 * - Create new import statement otherwise
 * - A new statement copies the file's quote and semicolon. A file with no
 *   import yet gets `'` and `;`. Merges leave the existing clause unchanged.
 * - `isTypeOnly` on a named specifier emits inline `type`, not `import type`
 * - A type-only binding does not count as the value. The same local name is
 *   promoted (`import { type Foo }` drops `type`; `import type { Foo, Bar }`
 *   becomes `import { Foo, type Bar }`). A different local name is a new statement.
 * - A named specifier already exists only when the same export and the same
 *   local name are bound. `foo as bar` does not satisfy `foo`, and
 *   `import * as ns` does not satisfy a named specifier.
 * - A new statement is inserted after the last module-level import. A
 *   `require()` or `import()` nested in a function is not an insertion anchor.
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
    const existing = getAllImports(program, { type: "default", from: options.from });
    if (existing.some((item) => !item.isNamespace && !item.isTypeOnly)) {
      return null; // Already has a value default import
    }
    const typeDefault = existing.find(
      (item) => !item.isNamespace && item.isTypeOnly && item.alias === options.name,
    );
    if (typeDefault) {
      const statement = (typeDefault.node as unknown as SgNode<TS>)
        .ancestors()
        .find((ancestor) => ancestor.kind() === "import_statement") as SgNode<TS> | undefined;
      const parts = statement
        ? analyzeImportClause(statement as unknown as SgNode<Language>)
        : null;
      if (statement && parts?.defaultIdent && !parts.namedImports && !parts.namespaceImport) {
        const promoted = dropStatementTypeKeyword(statement);
        if (promoted) return promoted;
      }
    }
  } else if (options.type === "namespace") {
    const existing = getAllImports(program, { type: "default", from: options.from });
    if (existing.some((item) => item.isNamespace)) {
      return null; // Already has a namespace import
    }
  } else if (options.type === "named") {
    // A value binding satisfies a value or type request. `import type` and
    // `import { type X }` do not satisfy a value request. Identity is the
    // exported name plus the local name: `foo as bar` is not `foo`. A
    // namespace import does not bind the named export.
    const pending: ImportSpecifier[] = [];
    for (const spec of options.specifiers) {
      if (!namedSpecifierExists(program, options.from, spec)) {
        pending.push(spec);
      }
    }

    if (pending.length === 0) {
      return null; // All specifiers already exist
    }

    const promoted = promotionEdit(program, options.from, pending);
    if (promoted) return promoted;

    // For ESM named imports, try to merge into existing import
    if (moduleType === "esm") {
      const merged = mergeNamedIntoExisting(program, options.from, pending);
      if (merged) return merged;
    }

    // Update options with filtered specifiers for new import creation
    options = { ...options, specifiers: pending };
  }

  // Find insertion position (after last module-level import, or at file start).
  // A require()/import() inside a function is not an anchor.
  const allImports = findAllImportStatements(program)
    .filter((statement) => isModuleLevelStatement(statement as unknown as SgNode<TS>))
    .sort((a, b) => a.range().start.index - b.range().start.index);
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

  // A type modifier is ESM syntax. A new statement copies the file's quotes
  // and semicolon; merges above do not reach this path.
  const style = getFileStyle(program);
  const createAsEsm =
    moduleType === "esm" ||
    (options.type === "named" && options.specifiers.some((spec) => spec.isTypeOnly));
  const importText = createAsEsm
    ? generateEsmImport(options, style)
    : generateCjsRequire(options, style);

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
 * `require('pkg');` / `import('pkg');` as a standalone expression statement (e.g. polyfill registration).
 */
function removeBareRequireImportSideEffectEdit(
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
    if (fn?.text() !== "require" && fn?.text() !== "import") {
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

function normalizeRemoveSpecifier(spec: RemoveNamedSpecifier): { name: string; alias?: string } {
  return typeof spec === "string" ? { name: spec } : spec;
}

function mergeRanges(
  ranges: Array<{ start: number; end: number }>,
): Array<{ start: number; end: number }> {
  const sorted = [...ranges].sort((left, right) => left.start - right.start);
  const merged: Array<{ start: number; end: number }> = [];
  for (const range of sorted) {
    const last = merged[merged.length - 1];
    if (!last || range.start > last.end) {
      merged.push({ start: range.start, end: range.end });
      continue;
    }
    last.end = Math.max(last.end, range.end);
  }
  return merged;
}

function utf8ByteLength(codePoint: number): number {
  if (codePoint <= 0x7f) return 1;
  if (codePoint <= 0x7ff) return 2;
  if (codePoint <= 0xffff) return 3;
  return 4;
}

/** AST ranges are UTF-8 byte offsets. `String#slice` counts UTF-16 code units. */
function sliceUtf8(source: string, startByte: number, endByte: number): string {
  if (endByte <= startByte) return "";
  let bytes = 0;
  let startIndex = startByte === 0 ? 0 : source.length;
  let endIndex = source.length;
  let foundStart = startByte === 0;
  for (let index = 0; index < source.length; ) {
    if (!foundStart && bytes >= startByte) {
      startIndex = index;
      foundStart = true;
    }
    if (foundStart && bytes >= endByte) {
      endIndex = index;
      break;
    }
    const code = source.charCodeAt(index);
    let point = code;
    let units = 1;
    if (code >= 0xd800 && code <= 0xdbff && index + 1 < source.length) {
      const next = source.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        point = (code - 0xd800) * 0x400 + (next - 0xdc00) + 0x10000;
        units = 2;
      }
    }
    bytes += utf8ByteLength(point);
    index += units;
  }
  if (!foundStart) startIndex = source.length;
  return source.slice(startIndex, endIndex);
}

function dropContainedEdits(edits: Edit[]): Edit[] {
  const sorted = [...edits].sort(
    (left, right) => left.startPos - right.startPos || right.endPos - left.endPos,
  );
  const kept: Edit[] = [];
  for (const edit of sorted) {
    const contained = kept.some(
      (outer) => outer.startPos <= edit.startPos && outer.endPos >= edit.endPos,
    );
    if (!contained) kept.push(edit);
  }
  return kept;
}

/**
 * One edit from the first start through the last end. Gaps are copied with
 * byte offsets. Callers use this inside a single clause, where the gap is
 * text that belongs to that edit.
 */
function spliceByteEdits(edits: Edit[], source: string): Edit | null {
  const sorted = dropContainedEdits(edits).sort((left, right) => left.startPos - right.startPos);
  const first = sorted[0];
  if (!first) return null;
  if (sorted.length === 1) return first;

  const start = first.startPos;
  const end = sorted.reduce((max, edit) => Math.max(max, edit.endPos), first.endPos);
  let cursor = start;
  let text = "";
  for (const edit of sorted) {
    if (edit.startPos > cursor) text += sliceUtf8(source, cursor, edit.startPos);
    text += edit.insertedText;
    cursor = Math.max(cursor, edit.endPos);
  }
  if (cursor < end) text += sliceUtf8(source, cursor, end);
  return { startPos: start, endPos: end, insertedText: text };
}

/**
 * Edits separated by other source stay separate so a later `commitEdits` can
 * still change the text between them. Contiguous edits collapse into one.
 * A deletion that contains another edit wins, and the inner edit is dropped.
 */
function separateEdits(edits: Edit[], source: string): Edit | Edit[] | null {
  const sorted = dropContainedEdits(edits).sort((left, right) => left.startPos - right.startPos);
  const first = sorted[0];
  if (!first) return null;

  const groups: Edit[][] = [[first]];
  for (const edit of sorted.slice(1)) {
    const group = groups[groups.length - 1]!;
    const last = group[group.length - 1]!;
    if (edit.startPos <= last.endPos) group.push(edit);
    else groups.push([edit]);
  }

  const coalesced = groups.map((group) =>
    group.length === 1 ? group[0]! : spliceByteEdits(group, source)!,
  );
  if (coalesced.length === 1) return coalesced[0]!;
  return coalesced;
}

function editDeletingRanges(
  programText: string,
  ranges: Array<{ start: number; end: number }>,
): Edit | null {
  const merged = mergeRanges(ranges);
  const first = merged[0];
  if (!first) return null;
  return spliceByteEdits(
    merged.map((range) => ({ startPos: range.start, endPos: range.end, insertedText: "" })),
    programText,
  );
}

const OBJECT_BINDING_KINDS = new Set([
  "shorthand_property_identifier_pattern",
  "pair_pattern",
  "object_assignment_pattern",
  "assignment_pattern",
  "rest_pattern",
]);

/** Includes defaulted properties such as `{ kept = 1 }`, which are not listed specifiers. */
function objectPatternBindingCount(pattern: SgNode<TS>): number {
  return pattern.children().filter((child) => OBJECT_BINDING_KINDS.has(child.kind())).length;
}

function statementBindingCount<T extends Language>(statement: SgNode<T>, listed: number): number {
  const pattern = (statement as unknown as SgNode<TS>).find({ rule: { kind: "object_pattern" } });
  if (!pattern) return listed;
  return Math.max(listed, objectPatternBindingCount(pattern));
}

function requireObjectPatterns<T extends Language>(
  program: SgNode<T, "program">,
  from: string,
): Array<SgNode<T>> {
  const tsProgram = program as unknown as SgNode<TS, "program">;
  const source = stringToExactRegexString(from);
  const value = {
    field: "value",
    any: [
      {
        all: [
          { kind: "call_expression" },
          { has: { field: "function", regex: "^(require|import)$" } },
          {
            has: {
              field: "arguments",
              has: {
                kind: "string",
                any: [{ regex: source }, { has: { kind: "string_fragment", regex: source } }],
              },
            },
          },
        ],
      },
      {
        kind: "await_expression",
        has: {
          all: [
            { kind: "call_expression" },
            { has: { field: "function", regex: "^import$" } },
            {
              has: {
                field: "arguments",
                has: {
                  kind: "string",
                  any: [{ regex: source }, { has: { kind: "string_fragment", regex: source } }],
                },
              },
            },
          ],
        },
      },
    ],
  } satisfies Rule<Language>;
  const patterns = new Map<number, SgNode<T>>();
  const specifiers = tsProgram.findAll({
    rule: {
      any: [{ kind: "shorthand_property_identifier_pattern" }, { kind: "pair_pattern" }],
      inside: {
        stopBy: "end",
        kind: "object_pattern",
        inside: {
          stopBy: "end",
          kind: "variable_declarator",
          has: value,
        },
      },
    },
  });
  for (const specifier of specifiers) {
    const pattern = specifier.ancestors().find((ancestor) => ancestor.kind() === "object_pattern");
    if (!pattern) continue;
    patterns.set(pattern.range().start.index, pattern as unknown as SgNode<T>);
  }
  return [...patterns.values()];
}

function removeDefaultBindingEdit<T extends Language>(
  statement: SgNode<T>,
  programText: string,
): Edit {
  if ((statement as unknown as SgNode<TS>).kind() === "import_statement") {
    const parts = analyzeImportClause(statement);
    const sibling = parts.namedImports ?? parts.namespaceImport;
    if (parts.defaultIdent && sibling) {
      return {
        startPos: parts.defaultIdent.range().start.index,
        endPos: sibling.range().start.index,
        insertedText: "",
      };
    }
  }

  const { start, end } = getStatementRangeWithNewline(statement, programText);
  return { startPos: start, endPos: end, insertedText: "" };
}

/**
 * Remove an import from the program. Smart behavior:
 * - Default/namespace: Removes entire import statement
 * - Named (multiple specifiers exist): Removes only the specified specifiers
 * - Named (removing last specifiers): Removes entire import statement
 * - Default + `var` + `require()`: removed (same as `const`/`let`)
 * - Default + `removeSideEffectForms`: also removes bare `require('m')` and `import 'm'` when there is no binding import
 * - The same export in a later statement is removed too. Pass `alias` (or
 *   `{ name, alias }` for a named specifier) to remove one local binding.
 * - Edits separated by other statements are returned as an array so they do
 *   not swallow the text between them.
 *
 * Note: removeImport returns null for multi-declarator CJS declarations
 * (e.g. `const foo = require('mod'), x = 1`). These are not tracked by
 * findAllImportStatements to prevent accidental deletion of unrelated bindings.
 *
 * @returns Edit, an array of non-overlapping edits, or null if import not found
 */
export function removeImport<T extends Language>(
  program: SgNode<T, "program">,
  options: RemoveImportOptions,
): Edit | Edit[] | null {
  const programText = program.text();
  const allStatements = findAllImportStatements(program);

  if (options.type === "default") {
    const stripSideEffects = options.removeSideEffectForms === true;
    if (options.alias) {
      const listed = listImports(program, { from: options.from }).find(
        (item) => item.defaultName === options.alias,
      );
      if (!listed) return null;
      return removeDefaultBindingEdit(listed.node as unknown as SgNode<T>, programText);
    }

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
      return removeDefaultBindingEdit(statement, programText);
    }

    if (stripSideEffects) {
      const tsProgram = program as unknown as SgNode<TS, "program">;
      return (
        removeBareRequireImportSideEffectEdit(tsProgram, options.from) ??
        removeSideEffectImportStatementEdit(tsProgram, options.from)
      );
    }

    return null;
  }

  if (options.type === "namespace") {
    // Locate the namespace_import directly by source. Going through getImport
    // would miss mixed statements like `import foo, * as ns from 'mod'`,
    // where the default match wins over the namespace fallback.
    const tsProgram = program as unknown as SgNode<TS, "program">;
    const namespaceMatches = tsProgram.findAll({
      rule: {
        kind: "namespace_import",
        inside: {
          stopBy: "end",
          kind: "import_statement",
          has: {
            field: "source",
            has: {
              kind: "string_fragment",
              regex: stringToExactRegexString(options.from),
            },
          },
        },
      },
    });
    const namespaceEsmImport =
      (options.alias
        ? namespaceMatches.find((node) =>
            node
              .children()
              .some((child) => child.kind() === "identifier" && child.text() === options.alias),
          )
        : namespaceMatches[0]) ?? null;

    if (namespaceEsmImport) {
      const statement =
        (namespaceEsmImport.ancestors().find((a) => a.kind() === "import_statement") as
          | SgNode<TS>
          | undefined) ?? null;
      if (!statement) return null;

      const parts = analyzeImportClause(statement);
      if (parts.defaultIdent && parts.namespaceImport) {
        // Mixed: strip ', * as ns' keeping the default binding.
        return {
          startPos: parts.defaultIdent.range().end.index,
          endPos: parts.namespaceImport.range().end.index,
          insertedText: "",
        };
      }

      const { start, end } = getStatementRangeWithNewline(
        statement as unknown as SgNode<Language>,
        programText,
      );
      return { startPos: start, endPos: end, insertedText: "" };
    }

    const dynamicNamespaces = listImports(program, { from: options.from }).filter(
      (item) =>
        item.namespaceName !== null &&
        (item.node as unknown as SgNode<TS>).kind() === "expression_statement",
    );
    const chosen = options.alias
      ? dynamicNamespaces.find((item) => item.namespaceName === options.alias)
      : dynamicNamespaces[0];
    if (!chosen) return null;

    const { start, end } = getStatementRangeWithNewline(
      chosen.node as unknown as SgNode<Language>,
      programText,
    );
    return { startPos: start, endPos: end, insertedText: "" };
  }

  if (options.type === "named") {
    const targets = options.specifiers.map(normalizeRemoveSpecifier);
    const edits: Edit[] = [];

    for (const item of listImports(program, { from: options.from })) {
      const matching = item.specifiers.filter((spec) =>
        targets.some(
          (target) =>
            target.name === spec.importedName &&
            (target.alias === undefined || target.alias === spec.localName),
        ),
      );
      if (matching.length === 0) continue;

      const statement = item.node as unknown as SgNode<T>;
      const removesEverySpecifier =
        matching.length >= item.specifiers.length &&
        matching.length >= statementBindingCount(statement, item.specifiers.length);
      if (removesEverySpecifier && !item.defaultName && !item.namespaceName) {
        const { start, end } = getStatementRangeWithNewline(statement, programText);
        edits.push({ startPos: start, endPos: end, insertedText: "" });
        continue;
      }

      if (
        removesEverySpecifier &&
        item.defaultName &&
        (statement as unknown as SgNode<TS>).kind() === "import_statement"
      ) {
        const parts = analyzeImportClause(statement);
        if (parts.defaultIdent && parts.namedImports) {
          edits.push({
            startPos: parts.defaultIdent.range().end.index,
            endPos: parts.namedImports.range().end.index,
            insertedText: "",
          });
          continue;
        }
      }

      const ranges = matching.map((spec) =>
        getSpecifierRangeWithSeparator(spec.node as unknown as SgNode<T>, programText),
      );
      const edit = editDeletingRanges(
        programText,
        ranges.map((range) => ({ start: range.start, end: range.end })),
      );
      if (edit) edits.push(edit);
    }

    return separateEdits(edits, programText);
  }

  return null;
}

function renameApplies(
  read: { name: string; alias?: string },
  rename: UpdateImportOptions["specifiers"][number],
): boolean {
  if (read.name !== rename.name) return false;
  const local = read.alias ?? read.name;
  return rename.local === undefined || rename.local === local;
}

function resultingLocalName(exportedName: string, alias: string | undefined): string {
  return alias && alias !== exportedName ? alias : exportedName;
}

/**
 * Replace named specifiers of an existing import/require in place.
 *
 * - `import { foo } from 'mod'` + `{ name: "foo", to: "bar" }` -> `import { bar } from 'mod'`
 * - Every clause that imports `name` is updated, not only the first statement.
 * - `local` selects one binding when the same export is imported more than once.
 * - Distinct aliases of one export stay (`foo` and `foo as bar`).
 * - If the resulting local name is already used in the same clause, the rename is dropped and the existing binding stays.
 * - Clauses separated by other statements come back as an array of edits.
 *
 * @returns Edit, an array of non-overlapping edits, or null if none of the specifiers is found
 */
export function updateImport<T extends Language>(
  program: SgNode<T, "program">,
  options: UpdateImportOptions,
): Edit | Edit[] | null {
  if (options.type !== "named" || options.specifiers.length === 0) {
    return null;
  }

  const clauses = new Map<number, SgNode<T>>();
  const rememberClause = (clause: SgNode<T> | null | undefined) => {
    if (!clause) return;
    clauses.set(clause.range().start.index, clause);
  };

  for (const item of listImports(program, { from: options.from })) {
    for (const spec of item.specifiers) {
      const read = readSpecifier(spec.node as unknown as SgNode<T>);
      if (!read || !options.specifiers.some((rename) => renameApplies(read, rename))) continue;
      const clause = (spec.node as unknown as SgNode<TS>)
        .ancestors()
        .find((ancestor) => ancestor.is("named_imports") || ancestor.is("object_pattern"));
      rememberClause(clause as unknown as SgNode<T> | undefined);
    }
  }

  // Multi-declarator requires are omitted from listImports so a later deletion
  // cannot drop the sibling. Renames still apply to that object pattern.
  for (const pattern of requireObjectPatterns(program, options.from)) {
    const hasRename = readClauseItems(pattern).some((item) => {
      if (item.kind !== "specifier") return false;
      const read = readSpecifier(item.node);
      return read !== null && options.specifiers.some((rename) => renameApplies(read, rename));
    });
    if (hasRename) rememberClause(pattern);
  }

  const edits: Edit[] = [];
  for (const clause of clauses.values()) {
    const existingItems = readClauseItems(clause);
    const kind = clause.kind() === "object_pattern" ? "object_pattern" : "named_imports";
    const retainedLocals = new Set<string>();
    for (const item of existingItems) {
      if (item.kind !== "specifier") continue;
      const read = readSpecifier(item.node);
      if (!read || options.specifiers.some((rename) => renameApplies(read, rename))) continue;
      retainedLocals.add(read.alias ?? read.name);
    }

    const emittedLocals = new Set<string>(retainedLocals);
    let changed = false;
    const items: ClauseItem[] = [];

    for (const item of existingItems) {
      if (item.kind === "comment") {
        items.push(item);
        continue;
      }

      const specifierNode = readSpecifier(item.node);
      if (!specifierNode) {
        items.push(item);
        continue;
      }

      const rename = options.specifiers.find((candidate) =>
        renameApplies(specifierNode, candidate),
      );
      if (!rename) {
        items.push(item);
        continue;
      }

      const alias = rename.alias ?? specifierNode.alias;
      const newLocal = resultingLocalName(rename.to, alias);
      if (emittedLocals.has(newLocal)) {
        changed = true;
        continue;
      }

      changed = true;
      emittedLocals.add(newLocal);
      const isImportType = item.node.child(0)?.kind() === "type";
      const newImportName = isImportType ? `type ${rename.to}` : rename.to;
      items.push({
        kind: "specifier",
        text: formatSpecifier({ name: newImportName, alias }, kind),
      });
    }

    if (changed) edits.push(serializeSpecifierClause(clause, items));
  }

  return separateEdits(edits, program.text());
}

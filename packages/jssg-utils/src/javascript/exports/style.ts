import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type { SgNode } from "@codemod.com/jssg-types/main";

type Language = JS | TS | TSX;

export interface FileStyle {
  /**
   * Quote of the first module specifier. When the file has none, quote of the
   * first string literal. `'` when the file has neither.
   */
  quote: string;
  /**
   * `";"` or `""` from that specifier's statement. With no specifier, from the
   * first semicolon-style statement. `";"` when the file has neither.
   */
  semicolon: string;
  /** Whitespace of one indent level. Two spaces when the file has no indented block. */
  indentUnit: string;
}

const FALLBACK_QUOTE = "'";
const FALLBACK_SEMICOLON = ";";
const FALLBACK_INDENT = "  ";

/**
 * `range().index` is a UTF-8 byte offset. String indexes are UTF-16 code units,
 * so a raw byte offset lands on the wrong character once non-ASCII text precedes it.
 */
function utf16IndexAtByte(src: string, byteOffset: number): number {
  let bytes = 0;
  let index = 0;
  while (index < src.length && bytes < byteOffset) {
    const code = src.charCodeAt(index);
    if (code < 0x80) {
      bytes += 1;
      index += 1;
    } else if (code < 0x800) {
      bytes += 2;
      index += 1;
    } else if (code >= 0xd800 && code <= 0xdbff) {
      bytes += 4;
      index += 2;
    } else {
      bytes += 3;
      index += 1;
    }
  }
  return index;
}

/**
 * Leading spaces or tabs of the line `node` starts on.
 * Returns `""` when any other character appears before the node on that line,
 * so a mid-line node is not given the code that precedes it as its indent.
 */
export function getLineIndent<T extends Language>(src: string, node: SgNode<T>): string {
  const start = utf16IndexAtByte(src, node.range().start.index);
  let lineStart = start;
  while (lineStart > 0 && src[lineStart - 1] !== "\n") {
    lineStart--;
  }
  const linePrefix = src.slice(lineStart, start);
  return /^[ \t]*$/.test(linePrefix) ? linePrefix : "";
}

/**
 * Prefix every non-empty line with `indent`. Blank lines stay blank.
 */
export function indentText(text: string, indent: string): string {
  return text
    .split("\n")
    .map((line) => (line === "" ? line : `${indent}${line}`))
    .join("\n");
}

function isQuote(char: string): boolean {
  return char === "'" || char === '"' || char === "`";
}

function semicolonOf(statement: SgNode<Language>): string {
  return statement.text().trimEnd().endsWith(";") ? ";" : "";
}

function quoteOf(stringNode: SgNode<Language>): string {
  const quote = stringNode.text().charAt(0);
  return isQuote(quote) ? quote : FALLBACK_QUOTE;
}

function enclosingStatement(node: SgNode<Language>): SgNode<Language> {
  const kind = node.kind();
  if (
    kind === "import_statement" ||
    kind === "lexical_declaration" ||
    kind === "variable_declaration" ||
    kind === "expression_statement"
  ) {
    return node;
  }

  const found = node.ancestors().find((ancestor) => {
    const ancestorKind = ancestor.kind();
    return (
      ancestorKind === "import_statement" ||
      ancestorKind === "lexical_declaration" ||
      ancestorKind === "variable_declaration" ||
      ancestorKind === "expression_statement"
    );
  });

  return (found as SgNode<Language> | undefined) ?? node;
}

interface StyleHit {
  index: number;
  quote: string;
  semicolon: string;
}

const SEMICOLON_STATEMENT_KINDS = [
  "expression_statement",
  "lexical_declaration",
  "variable_declaration",
  "return_statement",
  "throw_statement",
  "break_statement",
  "continue_statement",
  "debugger_statement",
  "export_statement",
] as const;

/**
 * Quote and semicolon of the first module specifier in the file.
 * With none, the first string literal and the first semicolon-style statement.
 * `'` and `;` remain only when the file has nothing to sample.
 */
function detectQuoteAndSemicolon<T extends Language>(
  program: SgNode<T, "program">,
): {
  quote: string;
  semicolon: string;
} {
  const hits: StyleHit[] = [];
  const root = program as unknown as SgNode<Language, "program">;

  for (const kind of ["import_statement", "export_statement"] as const) {
    for (const statement of root.findAll({ rule: { kind } })) {
      const typed = statement as unknown as SgNode<TS>;
      const sourceNode = typed.field("source");
      if (!sourceNode) continue;
      hits.push({
        index: typed.range().start.index,
        quote: quoteOf(sourceNode as SgNode<Language>),
        semicolon: semicolonOf(typed as SgNode<Language>),
      });
    }
  }

  const calls = root.findAll({
    rule: {
      kind: "call_expression",
      has: {
        field: "function",
        regex: "^(require|import)$",
      },
    },
  });

  for (const call of calls) {
    const args = call.field("arguments");
    if (!args) continue;

    let specifier: SgNode<Language> | null = null;
    for (const child of args.children()) {
      const kind = child.kind();
      if (kind === "(" || kind === ")" || kind === ",") continue;
      specifier = kind === "string" ? (child as SgNode<Language>) : null;
      break;
    }
    if (!specifier) continue;

    const statement = enclosingStatement(call as SgNode<Language>);
    hits.push({
      index: statement.range().start.index,
      quote: quoteOf(specifier),
      semicolon: semicolonOf(statement),
    });
  }

  hits.sort((left, right) => left.index - right.index);
  const first = hits[0];
  if (first) {
    return { quote: first.quote, semicolon: first.semicolon };
  }

  return {
    quote: firstStringQuote(root) ?? FALLBACK_QUOTE,
    semicolon: firstStatementSemicolon(root) ?? FALLBACK_SEMICOLON,
  };
}

/**
 * A declaration export ends with `}` and does not choose semicolon style.
 * An export clause does, including one that omits the semicolon.
 */
function exportStatesSemicolonStyle(statement: SgNode<Language>): boolean {
  if (statement.find({ rule: { kind: "export_clause" } })) return true;
  return !statement.text().trimEnd().endsWith("}");
}

function firstStringQuote(root: SgNode<Language>): string | null {
  let quote: string | null = null;
  let index = Number.POSITIVE_INFINITY;

  for (const node of root.findAll({ rule: { kind: "string" } })) {
    const start = node.range().start.index;
    if (start >= index) continue;
    index = start;
    quote = quoteOf(node as SgNode<Language>);
  }

  return quote;
}

function considerStatement(
  node: SgNode<Language>,
  best: { index: number; semicolon: string | null },
): void {
  if (node.kind() === "export_statement" && !exportStatesSemicolonStyle(node)) return;

  const start = node.range().start.index;
  if (start >= best.index) return;
  best.index = start;
  best.semicolon = semicolonOf(node);
}

function firstStatementSemicolon(root: SgNode<Language>): string | null {
  const best: { index: number; semicolon: string | null } = {
    index: Number.POSITIVE_INFINITY,
    semicolon: null,
  };

  for (const kind of SEMICOLON_STATEMENT_KINDS) {
    for (const node of root.findAll({ rule: { kind } })) {
      considerStatement(node as SgNode<Language>, best);
    }
  }

  // JavaScript has no type-alias node. A missing kind rejects the rule.
  try {
    for (const node of root.findAll({ rule: { kind: "type_alias_declaration" } })) {
      considerStatement(node as SgNode<Language>, best);
    }
  } catch {
    return best.semicolon;
  }

  return best.semicolon;
}

/**
 * One indent level, taken from the first non-comment statement in a block.
 * The block's own line indent is subtracted, so a nested block still yields
 * a single level. Tabs stay tabs.
 */
function detectIndentUnit<T extends Language>(program: SgNode<T, "program">): string {
  const root = program as unknown as SgNode<Language>;
  const source = root.text();
  const blocks = root.findAll({ rule: { kind: "statement_block" } });

  for (const block of blocks) {
    const firstStatement = block.children().find((child) => {
      return child.isNamed() && child.kind() !== "comment";
    });
    if (!firstStatement) continue;

    const blockIndent = getLineIndent(source, block as SgNode<Language>);
    const statementIndent = getLineIndent(source, firstStatement as SgNode<Language>);
    if (statementIndent.startsWith(blockIndent) && statementIndent.length > blockIndent.length) {
      return statementIndent.slice(blockIndent.length);
    }
  }

  return FALLBACK_INDENT;
}

/**
 * Quote, semicolon, and indent used by this file.
 * The first module specifier wins. With none, the first string and the first
 * semicolon-style statement do.
 */
export function getFileStyle<T extends Language>(program: SgNode<T, "program">): FileStyle {
  return {
    ...detectQuoteAndSemicolon(program),
    indentUnit: detectIndentUnit(program),
  };
}

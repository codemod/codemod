/**
 * Working out what a string expression is, as far as the code says: the
 * fixed text, and the places where a value is put in.
 */

type SgNode = any;

export type StringPart =
  /** Text known from the code. */
  | { text: string }
  /** A value put into the string; `node` is the expression. */
  | { node: SgNode }
  /** A string the analysis could not see into; `node` is where it stopped. */
  | { unknown: SgNode };

const MAX_DEPTH = 8;

/**
 * The parts of a string expression: literals, template literals, `a + b`,
 * and constants followed to their value (in the file, or through an import).
 *
 *     `/api/users/${id}`            [text "/api/users/", node id]
 *     BASE + "/users"               [text "/api", text "/users"]   (const BASE = "/api")
 *     url                           [unknown url]                  (a parameter)
 */
export function stringParts(node: SgNode, depth = 0): StringPart[] {
  if (!node) return [];
  const kind: string = node.kind();
  if (kind === "string") {
    return [{ text: node.children().filter((c: SgNode) => c.kind() === "string_fragment" || c.kind() === "escape_sequence").map((c: SgNode) => c.text()).join("") }];
  }
  if (kind === "template_string") {
    const parts: StringPart[] = [];
    for (const child of node.children()) {
      if (child.kind() === "string_fragment" || child.kind() === "escape_sequence") parts.push({ text: child.text() });
      else if (child.kind() === "template_substitution") {
        const inner = child.children().find((c: SgNode) => c.isNamed());
        parts.push(...substituted(inner, depth));
      }
    }
    return merge(parts);
  }
  if (kind === "binary_expression" && node.field("operator")?.text() === "+") {
    return merge([...substituted(node.field("left"), depth), ...substituted(node.field("right"), depth)]);
  }
  if (kind === "parenthesized_expression" || kind === "as_expression" || kind === "non_null_expression") {
    return stringParts(node.children().find((c: SgNode) => c.isNamed()), depth);
  }
  if (kind === "identifier" && depth < MAX_DEPTH) {
    const value = constantValue(node);
    if (value) return stringParts(value, depth + 1);
  }
  // The whole string comes from somewhere the code does not show.
  return [{ unknown: node }];
}

/**
 * A piece of a template or a concatenation: its own parts when the code
 * shows them, else one value put into the string.
 */
function substituted(node: SgNode, depth: number): StringPart[] {
  if (!node) return [];
  const parts = stringParts(node, depth + 1);
  return parts.length === 1 && "unknown" in parts[0] ? [{ node }] : parts;
}

function merge(parts: StringPart[]): StringPart[] {
  const out: StringPart[] = [];
  for (const part of parts) {
    const last = out[out.length - 1];
    if ("text" in part && last && "text" in last) last.text += part.text;
    else out.push("text" in part ? { text: part.text } : part);
  }
  return out;
}

/**
 * The one value a name always has: the initializer of a `const` (followed
 * through imports), or of a variable that is never assigned again.
 */
export function constantValue(name: SgNode): SgNode | null {
  let definition: any;
  try {
    definition = name.definition({ follow: true });
  } catch {
    return null;
  }
  const declarator = definition?.node?.kind() === "variable_declarator" ? definition.node : definition?.nameNode?.parent();
  if (!declarator || declarator.kind() !== "variable_declarator") return null;
  const value = declarator.field("value");
  if (!value) return null;
  const declaration = declarator.parent();
  const isConst = declaration?.children().some((c: SgNode) => c.text() === "const");
  if (!isConst) {
    try {
      const writes = declarator.field("name").mutations().filter((m: any) => m.kind === "reassign" || m.kind === "update");
      if (writes.length > 0) return null;
    } catch {
      return null;
    }
  }
  return value;
}

/** The string an expression always is, or null. */
export function constantString(node: SgNode): string | null {
  const parts = stringParts(node);
  return parts.every((part) => "text" in part) ? parts.map((part: any) => part.text).join("") : null;
}

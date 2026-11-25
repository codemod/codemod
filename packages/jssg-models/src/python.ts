/**
 * Reading Python syntax in a model: decorators, call arguments, parameters,
 * imports. Thin helpers over the Python tree-sitter grammar's node kinds.
 */

type SgNode = any;

const named = (node: SgNode | null | undefined): SgNode[] => node?.children().filter((child: SgNode) => child.isNamed()) ?? [];

export const python = {
  /**
   * Every decorated function with each of its decorators:
   * `decorator` is the expression after `@` (a call, an attribute or a name).
   */
  decorated(root: SgNode): { decorator: SgNode; fn: SgNode }[] {
    const found: { decorator: SgNode; fn: SgNode }[] = [];
    for (const definition of root.findAll({ rule: { kind: "decorated_definition" } })) {
      const fn = definition.field("definition");
      if (fn?.kind() !== "function_definition") continue;
      for (const child of definition.children()) {
        if (child.kind() !== "decorator") continue;
        const decorator = named(child)[0];
        if (decorator) found.push({ decorator, fn });
      }
    }
    return found;
  },

  /** A call's positional arguments. */
  positional(call: SgNode): SgNode[] {
    return named(call.field("arguments")).filter((argument) => argument.kind() !== "keyword_argument" && argument.kind() !== "comment");
  },

  /** A call's keyword arguments, in order: `[name, value]`. */
  keywords(call: SgNode): [string, SgNode][] {
    return named(call.field("arguments"))
      .filter((argument) => argument.kind() === "keyword_argument")
      .map((argument) => [argument.field("name").text(), argument.field("value")]);
  },

  /** The value of one keyword argument, or undefined. */
  keyword(call: SgNode, name: string): SgNode | undefined {
    return python.keywords(call).find(([key]) => key === name)?.[1];
  },

  /** The items of a list or tuple literal. */
  elements(list: SgNode): SgNode[] {
    return list.kind() === "list" || list.kind() === "tuple" ? named(list) : [];
  },

  /** A function's parameters, as the name each one binds. */
  parameters(fn: SgNode): SgNode[] {
    return named(fn.field("parameters"))
      .map((parameter) => {
        if (parameter.kind() === "identifier") return parameter;
        // `x: int`, `x=1`, `x: int = 1`: the name is the first identifier.
        return parameter.field("name") ?? named(parameter).find((child) => child.kind() === "identifier");
      })
      .filter(Boolean);
  },

  /**
   * What each imported name stands for, as a dotted path:
   * `from tasks.mail import send as s` → `s` is `tasks.mail.send`;
   * `import tasks.mail as m` → `m` is `tasks.mail`.
   */
  imports(root: SgNode): Map<string, string> {
    const names = new Map<string, string>();
    for (const statement of root.findAll({ rule: { kind: "import_from_statement" } })) {
      const module = statement.field("module_name")?.text() ?? "";
      for (const child of statement.fieldChildren("name")) {
        if (child.kind() === "dotted_name") names.set(child.text(), `${module}.${child.text()}`);
        if (child.kind() === "aliased_import") {
          names.set(child.field("alias").text(), `${module}.${child.field("name").text()}`);
        }
      }
    }
    for (const statement of root.findAll({ rule: { kind: "import_statement" } })) {
      for (const child of named(statement)) {
        if (child.kind() === "dotted_name") names.set(child.text(), child.text());
        if (child.kind() === "aliased_import") names.set(child.field("alias").text(), child.field("name").text());
      }
    }
    return names;
  },
};

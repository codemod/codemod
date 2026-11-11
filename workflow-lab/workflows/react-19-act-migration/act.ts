import type { SgNode, SgRoot } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";

/** `import ... from 'react-dom/test-utils'`: React 19 moved `act` to `react` and removed the rest. */
export function testUtilsImports(root: SgRoot<TSX>): SgNode<TSX>[] {
  return root.root().findAll({
    rule: {
      kind: "import_statement",
      has: { field: "source", regex: "^['\"]react-dom/test-utils['\"]$" },
    },
  });
}

/** Only `import { act } from 'react-dom/test-utils'` is mechanical; anything else is left for a person. */
export function isActOnly(node: SgNode<TSX>): boolean {
  const specifiers = node.findAll({ rule: { kind: "import_specifier" } });
  const hasOther = node.has({
    rule: {
      any: [
        { kind: "namespace_import" },
        { kind: "identifier", inside: { kind: "import_clause" } },
      ],
    },
  });
  return (
    !hasOther &&
    specifiers.length > 0 &&
    specifiers.every((s) => s.field("name")?.text() === "act" && !s.field("alias"))
  );
}

export function rewrite(node: SgNode<TSX>) {
  const source = node.field("source")!;
  const quote = source.text()[0];
  return source.replace(`${quote}react${quote}`);
}

import { ok as assert } from "assert";

export type SemanticCodemodRoot = {
  root(): any;
};

export type SemanticCodemodNode = any;

export function requireNode<T>(node: T | null | undefined, message: string): T {
  assert(node != null, message);
  return node;
}

export function findIdentifierWithAncestorKind(
  program: SemanticCodemodNode,
  name: string,
  ancestorKind: string,
) {
  const astNode = program as any;

  return (
    astNode
      .findAll({
        rule: {
          kind: "identifier",
          pattern: name,
        },
      })
      .find((node: any) =>
        node.ancestors().some((ancestor: any) => String(ancestor.kind()) === ancestorKind),
      ) ?? null
  );
}

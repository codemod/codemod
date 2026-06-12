import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";

export type JavaNode = SgNode<Java>;

export type JavaMethodInvocationParts = {
  receiver: JavaNode | null;
  methodName: string | null;
  nameNode: JavaNode | null;
  args: JavaNode[];
};

export function getJavaMethodInvocationParts(
  invocation: JavaNode,
): JavaMethodInvocationParts | null {
  if (invocation.kind() !== "method_invocation") {
    return null;
  }

  const nameNode = invocation.field("name");
  const args =
    invocation
      .field("arguments")
      ?.children()
      .filter((child) => child.isNamed()) ?? [];

  return {
    receiver: invocation.field("object"),
    methodName: nameNode?.text() ?? null,
    nameNode,
    args,
  };
}

export function getJavaReceiverIdentifier(receiver: JavaNode | null): string | null {
  if (!receiver) {
    return null;
  }

  const trimmed = receiver.text().trim();
  if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(trimmed)) {
    return trimmed;
  }

  const match = /(?:this\.)?([A-Za-z_][A-Za-z0-9_]*)$/.exec(trimmed);
  return match?.[1] ?? null;
}

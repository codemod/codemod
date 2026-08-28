import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";

export type JavaNode = SgNode<Java>;

export type MethodInvocationParts = {
  receiver: JavaNode | null;
  methodName: string | null;
  nameNode: JavaNode | null;
  args: JavaNode[];
};

export function getMethodInvocationParts(invocation: JavaNode): MethodInvocationParts | null {
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

export function getReceiverIdentifier(receiver: JavaNode | null): string | null {
  if (!receiver) {
    return null;
  }

  if (receiver.kind() === "identifier") {
    return receiver.text();
  }

  const field = receiver.field("field");
  if (field?.kind() === "identifier") {
    return field.text();
  }

  return null;
}

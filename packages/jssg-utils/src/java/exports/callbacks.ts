import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";

export type JavaNode = SgNode<Java>;

export function rewriteAnonymousJavaCallbackToWhenComplete(options: {
  receiverText: string;
  callbackNode: JavaNode;
  successMethod?: string;
  failureMethod?: string;
}): string | null {
  const {
    receiverText,
    callbackNode,
    successMethod = "onSuccess",
    failureMethod = "onFailure",
  } = options;

  if (callbackNode.kind() !== "object_creation_expression") {
    return null;
  }

  const classBody = callbackNode.find({ rule: { kind: "class_body" } });
  if (!classBody) {
    return null;
  }

  let success: JavaNode | null = null;
  let failure: JavaNode | null = null;

  for (const method of classBody.findAll({
    rule: { kind: "method_declaration" },
  })) {
    const methodName = method
      .children()
      .find((child) => child.kind() === "identifier")
      ?.text();
    if (methodName === successMethod) {
      success = method;
    }
    if (methodName === failureMethod) {
      failure = method;
    }
  }

  if (!success || !failure) {
    return null;
  }

  const successParam = getSingleJavaParameterName(success);
  const failureParam = getSingleJavaParameterName(failure);
  const successBody = getJavaMethodBodyContent(success);
  const failureBody = getJavaMethodBodyContent(failure);

  if (!successParam || !failureParam || successBody === null || failureBody === null) {
    return null;
  }

  const resultParam = successParam === failureParam ? `${successParam}Result` : successParam;
  const errorParam = failureParam;
  const rewrittenSuccessBody =
    successParam === resultParam
      ? successBody
      : replaceJavaIdentifier(successBody, successParam, resultParam);

  return `${receiverText}.whenComplete((${resultParam}, ${errorParam}) -> {\n    if (${errorParam} != null) {\n${indentJavaBlockContent(failureBody, 6)}\n    } else {\n${indentJavaBlockContent(rewrittenSuccessBody, 6)}\n    }\n  })`;
}

export function getSingleJavaParameterName(method: JavaNode): string | null {
  const params = method.find({ rule: { kind: "formal_parameters" } });
  const parameter = params?.find({ rule: { kind: "formal_parameter" } });
  if (!parameter) {
    return null;
  }

  const nameNode = parameter
    .children()
    .filter((child) => child.isNamed())
    .reverse()
    .find((child) => child.kind() === "identifier");
  return nameNode?.text() ?? null;
}

export function getJavaMethodBodyContent(method: JavaNode): string | null {
  const block = method.find({ rule: { kind: "block" } });
  if (!block) {
    return null;
  }

  const text = block.text();
  const trimmed = text.trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) {
    return null;
  }

  return trimmed.slice(1, -1).trim();
}

export function replaceJavaIdentifier(source: string, from: string, to: string): string {
  const escaped = from.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return source.replace(new RegExp(`\\b${escaped}\\b`, "g"), to);
}

function indentJavaBlockContent(content: string, spaces: number): string {
  const indentation = " ".repeat(spaces);
  return content
    .split("\n")
    .map((line) => `${indentation}${line.trim()}`)
    .join("\n");
}

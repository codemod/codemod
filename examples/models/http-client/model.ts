/**
 * HTTP clients: `fetch(url, init)` and axios-style instances
 * (`axios.get(url)`, `const api = axios.create({ baseURL }); api.post(url, body)`).
 *
 * Each call is the producer of the HTTP channel `METHOD /path`. The path is
 * read from the code as far as it goes: constants, `a + b`, template
 * literals. A value put into the path (`${id}`) is sent as that segment; a
 * URL the code does not show gives an open key, which is reported and joined
 * to nothing.
 */

import { defineModel, http, stringParts, constantString } from "@jssg/models";
import type { SgNode, StringPart } from "@jssg/models";

const VERBS = ["get", "post", "put", "patch", "delete", "head", "options"];
/** axios verbs whose second argument is the request body. */
const WITH_BODY = ["post", "put", "patch"];

function args(call: SgNode): SgNode[] {
  return (
    call
      .field("arguments")
      ?.children()
      .filter((child: SgNode) => child.isNamed()) ?? []
  );
}

/** The value of `name` in an object literal, or undefined. */
function property(object: SgNode | undefined, name: string): SgNode | undefined {
  if (!object || object.kind() !== "object") return undefined;
  for (const member of object.children()) {
    if (member.kind() === "pair" && member.field("key")?.text().replace(/['"]/g, "") === name) return member.field("value");
    if (member.kind() === "shorthand_property_identifier" && member.text() === name) return member;
  }
  return undefined;
}

/** `JSON.stringify(x)` sends `x`. */
function payload(body: SgNode | undefined): SgNode | undefined {
  if (body?.kind() === "call_expression" && body.field("function")?.text() === "JSON.stringify") return args(body)[0];
  return body;
}

/**
 * Whether `receiver` is axios or an instance made by `axios.create(...)`,
 * and the instance's `baseURL` if it has one. Follows the value back
 * through variables and imports.
 */
function axiosInstance(receiver: SgNode): { base: StringPart[] } | null {
  if (receiver.text() === "axios") return { base: [] };
  let definition: any;
  try {
    definition = receiver.definition({ follow: true });
  } catch {
    return null;
  }
  const declarator = definition?.nameNode?.parent();
  const created = declarator?.kind() === "variable_declarator" ? declarator.field("value") : null;
  if (created?.kind() !== "call_expression" || created.field("function")?.text() !== "axios.create") return null;
  const baseURL = property(args(created)[0], "baseURL");
  return { base: baseURL ? stringParts(baseURL) : [] };
}

export default defineModel({
  name: "http-client",
  version: "0.1.0",
  files: ["**/*.{ts,tsx,js,jsx,mjs}"],
  contains: ["fetch(", "axios", ...VERBS.map((verb) => `.${verb}(`)],
  model({ root, produce, tag }) {
    for (const call of root.root().findAll({ rule: { pattern: "fetch($$$ARGS)" } })) {
      const [url, init] = args(call);
      if (!url) continue;
      const methodNode = property(init, "method");
      const method = methodNode ? constantString(methodNode) : "GET";
      // A method the code does not show could be any: leave the call alone.
      if (!method) continue;
      const { key, sends } = http.request(method, stringParts(url));
      const body = payload(property(init, "body"));
      if (body) sends.body = body;
      produce(key, call, { sends, receives: { response: call } });
      tag(call, "http:client");
    }

    for (const call of root.root().findAll({ rule: { kind: "call_expression", has: { field: "function", kind: "member_expression" } } })) {
      const callee = call.field("function");
      const verb = callee.field("property")?.text();
      if (!VERBS.includes(verb)) continue;
      const instance = axiosInstance(callee.field("object"));
      if (!instance) continue;
      const [url, second] = args(call);
      if (!url) continue;
      const { key, sends } = http.request(verb, stringParts(url), instance.base);
      if (WITH_BODY.includes(verb) && second) sends.body = second;
      produce(key, call, { sends, receives: { response: call } });
      tag(call, "http:client");
    }
  },
});

/**
 * Next.js App Router route handlers.
 *
 * `app/api/users/[id]/route.ts` exporting `GET` means: Next calls `GET` for
 * `GET /api/users/:id`. Nothing in the file says so; this model does.
 *
 * - each exported method is an entry point (Next is its caller);
 * - it is the consumer of the HTTP channel `METHOD /route`;
 * - the request, and the route's `params`, carry what the client sent.
 */

import { defineModel, http } from "@jssg/models";
import type { SgNode } from "@jssg/models";

/** `apps/web/src/app/(shop)/api/users/[id]/route.ts` → `/api/users/[id]`. */
export function routeOf(relativePath: string): string | null {
  const found = relativePath.match(/(?:^|\/)app\/(.*\/)?route\.[cm]?[jt]sx?$/);
  if (!found) return null;
  const segments = (found[1] ?? "")
    .split("/")
    // Route groups `(shop)` and parallel-route slots `@modal` are not in the URL.
    .filter((part) => part !== "" && !/^\(.*\)$/.test(part) && !part.startsWith("@"));
  return `/${segments.join("/")}`;
}

/** The exported functions named after HTTP methods: `[method, function node]`. */
function handlers(root: SgNode): [string, SgNode][] {
  const found: [string, SgNode][] = [];
  for (const statement of root.findAll({ rule: { kind: "export_statement" } })) {
    const declaration = statement.field("declaration");
    if (!declaration) continue;
    if (declaration.kind() === "function_declaration") {
      const name = declaration.field("name")?.text();
      if (http.METHODS.includes(name)) found.push([name, declaration]);
    } else if (declaration.kind() === "lexical_declaration") {
      for (const declarator of declaration.findAll({ rule: { kind: "variable_declarator" } })) {
        const name = declarator.field("name")?.text();
        const value = declarator.field("value");
        if (http.METHODS.includes(name) && value && /function|arrow/.test(value.kind())) found.push([name, value]);
      }
    }
  }
  return found;
}

/** A function's parameters as binding patterns (TypeScript wraps each one). */
function parameters(fn: SgNode): SgNode[] {
  const list = fn.field("parameters");
  if (!list) return [];
  return list
    .children()
    .filter((child: SgNode) => child.isNamed())
    .map((child: SgNode) => child.field("pattern") ?? child);
}

/** Where a handler reads the route's params: `{ params }`, or `context.params`. */
function paramsNodes(fn: SgNode, context: SgNode | undefined): SgNode[] {
  if (!context) return [];
  if (context.kind() === "object_pattern") {
    const found: SgNode[] = [];
    for (const child of context.children()) {
      if (child.kind() === "shorthand_property_identifier_pattern" && child.text() === "params") found.push(child);
      else if (child.kind() === "pair_pattern" && child.field("key")?.text() === "params") found.push(child.field("value"));
    }
    return found;
  }
  const name = context.text();
  return fn.findAll({ rule: { pattern: `${name}.params` } });
}

export default defineModel({
  name: "next-app-routes",
  version: "0.1.0",
  files: ["**/app/**/route.{ts,tsx,js,jsx,mjs}", "app/**/route.{ts,tsx,js,jsx,mjs}"],
  model({ root, file, entryPoint, consume, tag }) {
    const route = routeOf(file.relative);
    if (route === null) return;

    for (const [method, fn] of handlers(root.root())) {
      const key = http.key(method, route);
      entryPoint(fn, { kind: "http", label: `${method} ${route}` });

      const [request, context] = parameters(fn);
      const params = paramsNodes(fn, context);
      const receives: Record<string, SgNode[]> = {};
      for (const name of http.params(key)) receives[name] = params;
      if (request) {
        // `await request.json()` is what the client put in the body.
        const requestName = request.text();
        receives.body = ["json", "text", "formData"].flatMap((reader) =>
          fn.findAll({ rule: { pattern: `${requestName}.${reader}()` } }),
        );
        tag(request, "source:user-input", "http:request");
      }
      for (const node of params) tag(node, "source:user-input", "http:route-params");

      // What the handler answers with goes back to the caller.
      const response = ["Response.json($BODY $$$REST)", "NextResponse.json($BODY $$$REST)"].flatMap((pattern) =>
        fn.findAll({ rule: { pattern } }).map((call: SgNode) => call.getMatch("BODY")),
      );

      consume(key, fn, { receives, sends: { response: response.filter(Boolean) } });
    }
  },
});

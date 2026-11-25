/**
 * Flask routes (Python).
 *
 *     bp = Blueprint("apps", __name__, url_prefix="/console/api")
 *
 *     @bp.route("/apps/<uuid:app_id>/run", methods=["POST"])
 *     def run_app(app_id): ...
 *
 * Flask calls `run_app` for `POST /console/api/apps/:app_id/run`: an entry
 * point, and the consumer of that HTTP channel. The route's parameters
 * arrive in the function's parameters of the same name.
 *
 * The model is TypeScript; the files it reads are Python. A `fetch` in a
 * TypeScript frontend joins these routes like any other HTTP consumer.
 */

import { constantString, defineModel, http, python } from "@jssg/models";
import type { SgNode } from "@jssg/models";

const SHORTCUTS = ["get", "post", "put", "patch", "delete"];

/** `url_prefix` of each `Blueprint(...)` in the file, by variable name. */
function blueprints(root: SgNode): Map<string, string> {
  const prefixes = new Map<string, string>();
  for (const assignment of root.findAll({ rule: { kind: "assignment" } })) {
    const value = assignment.field("right");
    if (value?.kind() !== "call" || !/(^|\.)Blueprint$/.test(value.field("function")?.text() ?? "")) continue;
    const prefix = python.keyword(value, "url_prefix");
    prefixes.set(assignment.field("left").text(), (prefix && constantString(prefix)) ?? "");
  }
  return prefixes;
}

export default defineModel({
  name: "flask-routes",
  version: "0.1.0",
  files: ["**/*.py"],
  contains: [".route(", ...SHORTCUTS.map((verb) => `.${verb}(`)],
  model({ root, entryPoint, consume, tag }) {
    const prefixes = blueprints(root.root());
    for (const { decorator, fn } of python.decorated(root.root())) {
      const call = decorator.kind() === "call" ? decorator : null;
      const callee = call?.field("function");
      if (callee?.kind() !== "attribute") continue;
      const verb = callee.field("attribute").text();
      if (verb !== "route" && !SHORTCUTS.includes(verb)) continue;

      const pathNode = python.positional(call)[0];
      const path = pathNode && constantString(pathNode);
      // A path the code does not show: nothing to say about this route.
      if (path === null || path === undefined) continue;
      const methods =
        verb === "route"
          ? (python.keyword(call, "methods")?.findAll({ rule: { kind: "string" } }).map((m: SgNode) => constantString(m)) ?? ["GET"])
          : [verb];
      const route = `${prefixes.get(callee.field("object").text()) ?? ""}${path}`;
      const parameters = python.parameters(fn);

      for (const method of methods) {
        if (!method) continue;
        const key = http.key(method, route);
        entryPoint(fn, { kind: "http", label: `${method.toUpperCase()} ${route}` });
        const receives: Record<string, SgNode[]> = {};
        for (const name of http.params(key)) {
          receives[name] = parameters.filter((parameter) => parameter.text() === name);
        }
        receives.body = [
          ...fn.findAll({ rule: { pattern: "request.get_json()" } }),
          ...fn.findAll({ rule: { pattern: "request.json" } }),
        ];
        const response = fn.findAll({ rule: { pattern: "jsonify($BODY)" } }).map((found: SgNode) => found.getMatch("BODY"));
        consume(key, fn, { receives, sends: { response: response.filter(Boolean) } });
        for (const node of Object.values(receives).flat()) tag(node, "source:user-input", "http:request");
      }
    }
  },
});

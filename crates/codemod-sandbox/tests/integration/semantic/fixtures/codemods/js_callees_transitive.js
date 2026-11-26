// "Functions transitively called from handler", across modules: the callees
// lookup, crawled; and the callers lookup, crawled back from db.js's connect().
import { callGraph, crawl } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function basename(path) {
  return path.split("/").pop();
}

// A function's name: its own, or the name it is bound to.
function nameOf(fn) {
  const own = fn.field("name");
  if (own) return own.text();
  const parent = fn.parent();
  const bound = parent && (parent.field("name") || parent.field("key"));
  return bound ? bound.text() : "<anonymous>";
}

// Hops back to a start node along `from`.
function depthOf(reached, node) {
  let depth = 0;
  for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) depth++;
  return depth;
}

export default async function transform(root) {
  const { callees, callers } = callGraph("javascript");
  const handler = root.root().find({ rule: { kind: "function_declaration" } });
  if (!handler) throw new Error("handler not found");

  // Only functions in the workspace are callees: Number(), log() and
  // res.json() have no edge.
  expectEqual(
    callees
      .get(handler)
      .map((e) => nameOf(e.node) + "@" + basename(e.node.getRoot().filename()))
      .sort(),
    ["createUser@users.js", "query@db.js"],
    "direct callees",
  );

  const all = await crawl(callees, handler);
  const reached = [...all].filter(([, edge]) => edge);
  expectEqual(
    reached
      .map(
        ([node, edge]) =>
          nameOf(node) +
          "@" +
          basename(node.getRoot().filename()) +
          ":" +
          edge.via +
          ":" +
          depthOf(all, node),
      )
      .sort(),
    [
      "connect@db.js:direct:2",
      "createUser@users.js:direct:1",
      "normalize@users.js:callback:2",
      "query@db.js:direct:1",
      "save@db.js:method:2",
      "validate@validate.js:direct:2",
    ],
    "transitive callees",
  );

  // createUser is imported through the services/index.js barrel; the edge
  // points at the arrow function in users.js, and at the call that reaches it.
  const [createUser, createUserEdge] = reached.find(([node]) => nameOf(node) === "createUser");
  if (createUser.kind() !== "arrow_function") {
    throw new Error("expected the arrow function, got " + createUser.kind());
  }
  if (createUserEdge.site.text() !== "createUser(label, email)") {
    throw new Error("unexpected call site " + createUserEdge.site.text());
  }

  // The same graph, walked the other way from db.js's connect(). The crawl
  // reaches each caller once, at its shortest depth.
  const [save] = reached.find(([node]) => nameOf(node) === "save");
  const connectCall = save.find({ rule: { pattern: "connect()" } });
  const up = await crawl(callers, connectCall);
  expectEqual(
    [...up]
      .filter(([, edge]) => edge)
      .map(([node]) => nameOf(node) + ":" + depthOf(up, node))
      .sort(),
    ["createUser:2", "handler:2", "query:1", "save:1"],
    "transitive callers of connect",
  );
  // handler, reached first through query, also calls createUser (depth 3
  // that way): the callers lookup still has that edge.
  const [upCreateUser] = [...up].find(([node]) => nameOf(node) === "createUser");
  expectEqual(
    callers.get(upCreateUser).map((e) => nameOf(e.node)),
    ["handler"],
    "callers of createUser",
  );

  return null;
}

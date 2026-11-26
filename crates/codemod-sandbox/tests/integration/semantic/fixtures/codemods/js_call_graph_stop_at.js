// crawl(callees, start, { stopAt }): nodes in stopAt are reached, and the
// crawl does not walk past them. Stopping at what a callback or method edge
// reached leaves those edges' targets listed and the rest of the walk as is.
import { callGraph, crawl } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
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
  const { callees } = callGraph("javascript");
  const handler = root.root().find({ rule: { kind: "function_declaration" } });
  const describe = (reached) =>
    [...reached]
      .filter(([, edge]) => edge)
      .map(([node, edge]) => nameOf(node) + ":" + edge.via + ":" + depthOf(reached, node))
      .sort();
  const all = await crawl(callees, handler);
  const reachedVia = (via) => [...all].filter(([, edge]) => edge?.via === via).map(([node]) => node);

  // Stop at callbacks: normalize is listed, and the rest of the walk is
  // unchanged.
  const callbacks = reachedVia("callback");
  expectEqual(callbacks.map(nameOf), ["normalize"], "reached through a callback");
  expectEqual(
    describe(await crawl(callees, handler, { stopAt: callbacks })),
    [
      "connect:direct:2",
      "createUser:direct:1",
      "normalize:callback:2",
      "query:direct:1",
      "save:method:2",
      "validate:direct:2",
    ],
    "stopAt callback",
  );

  // Stop at method calls: db.save is listed, and connect (called from save)
  // is not reached through it, only from query (direct) at depth 2.
  const methods = reachedVia("method");
  const noMethods = await crawl(callees, handler, { stopAt: methods });
  const listed = describe(noMethods);
  if (!listed.includes("save:method:2")) throw new Error("save should be listed");
  const connect = [...noMethods.keys()].find((node) => nameOf(node) === "connect");
  expectEqual(
    [nameOf(noMethods.get(connect).from), depthOf(noMethods, connect)],
    ["query", 2],
    "connect is reached from query, not through save: " + JSON.stringify(listed),
  );
  return null;
}

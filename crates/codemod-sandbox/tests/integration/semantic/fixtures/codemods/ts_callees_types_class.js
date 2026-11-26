// x.save() where x: UserRepo, a class: the callees and callers lookups follow
// the type, and the typeOf lookup gives the checker's type of an expression.
// Needs a TypeScript 7 checker (CODEMOD_TS_API).
import { callGraph, crawl, types } from "codemod:ast-grep";

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

// Hops back to a start node along `from`.
function depthOf(reached, node) {
  let depth = 0;
  for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) depth++;
  return depth;
}

export default async function transform(root) {
  const { callees, callers } = callGraph("typescript");
  const { typeOf } = types("typescript");
  const name = (node) => node.field("name")?.text();
  const register = root.root().find({ rule: { kind: "function_declaration" } });
  const call = register.find({ rule: { pattern: "repo.save($$$)" } });

  // The call resolves to the class method, marked as found through types.
  expectEqual(
    callees.get(call).map((e) => [name(e.node), basename(e.node.getRoot().filename()), e.via]),
    [["save", "repo.ts", "type"]],
    "callees of repo.save()",
  );
  // And a crawl continues through it.
  const all = await crawl(callees, register);
  expectEqual(
    [...all]
      .filter(([node, edge]) => edge && name(node) === "audit")
      .map(([node, edge]) => [name(node), edge.via, depthOf(all, node)]),
    [["audit", "direct", 2]],
    "crawled callees through the typed edge",
  );
  // Callers of the method see the typed call too.
  const save = callees.get(call)[0].node;
  expectEqual(
    callers.get(save).map((e) => [name(e.node), e.via]),
    [["register", "type"]],
    "callers of UserRepo.save",
  );
  // Stopping at what typed edges reached leaves the rest out of the walk.
  const typed = [...all].filter(([, edge]) => edge?.via === "type").map(([node]) => node);
  const untyped = await crawl(callees, register, { stopAt: typed });
  expectEqual(
    [...untyped.keys()].filter((node) => name(node) === "audit"),
    [],
    "stopAt what typed edges reached",
  );

  // typeOf: the checker's type for an expression.
  const repo = call.find({ rule: { pattern: "repo" } });
  expectEqual(typeOf.get(repo), "UserRepo", "type of repo");
  expectEqual(typeOf.get(call), "void", "type of repo.save(...)");
  const arg = call.find({ rule: { kind: "object" } });
  expectEqual(typeOf.get(arg), "{ id: string; }", "type of the argument");
  return null;
}

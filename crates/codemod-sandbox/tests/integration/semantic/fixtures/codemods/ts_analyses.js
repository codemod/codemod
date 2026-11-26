// Analyses as values: built-in lookups, lookup(edges), crawl() and join().
import { callGraph, crawl, join, lookup } from "codemod:ast-grep";
import { equal, test, throws } from "./spec.js";
export { run as default } from "./spec.js";

const profile = (root) => root.root().find("function Profile() { $$$ }");
const format = (root) => root.root().find("function format($$$) { $$$ }");
const fnName = (n) => n.text().match(/function (\w+)/)?.[1];
// tRPC procedures: the call trpc.user.getUser…(…) to the router's getUser:
// publicProcedure… pair.
const routes = () =>
  join({
    language: "typescript",
    from: { find: "trpc.$R.$P.$M($$$)", anchor: "function", on: "$P" },
    to: {
      find: {
        rule: { pattern: { context: "({ $P: publicProcedure.$M($$$) })", selector: "pair" } },
      },
      on: "$P",
    },
    meta: { via: "trpc" },
  });

test("a built-in lookup answers get(node) with the edges leaving the node", (root) => {
  equal(
    callGraph("typescript")
      .callees.get(profile(root))
      .map((e) => e.node.text().split("\n")[0] + " @ " + e.site.text()),
    ["function format(value: unknown) { @ format(user)"],
  );
  equal(
    callGraph("typescript").callers.get(format(root))[0].node.key(),
    profile(root).key(),
    "callers",
  );
});

test("a node without edges is undefined in get() and false in has(), like a Map", (root) => {
  const { callers } = callGraph("typescript");
  equal([callers.get(profile(root)), callers.has(profile(root))], [undefined, false]);
});

test("a built-in lookup can't be listed", async () => {
  const { callers } = callGraph("typescript");
  for (const list of [() => callers.size, () => [...callers], () => callers.keys()]) {
    const error = await throws(list, "get(node)");
    equal(error instanceof TypeError, true);
  }
});

test("lookup(edges) keys edges by syntax node, so another query's node finds them", (root) => {
  const renders = lookup([[profile(root), format(root), { why: "renders" }]]);
  const [edge] = renders.get(profile(root));
  equal([edge.why, edge.node.key()], ["renders", format(root).key()]);
  equal([renders.size, [...renders.keys()][0].key()], [1, profile(root).key()], "listing");
});

test("lookup(edges) takes nodes only, and names the wrong edge", async (root) => {
  const error = await throws(
    () =>
      lookup([
        [profile(root), profile(root)],
        [profile(root), "format"],
      ]),
    "edge 1",
  );
  equal(error instanceof TypeError, true);
});

test("crawl walks a lookup from a start node, with the edge that reached each node", async (root) => {
  const reached = await crawl(callGraph("typescript").callees, profile(root));
  equal(
    [...reached].map(
      ([node, edge]) => fnName(node) + " <- " + (edge ? fnName(edge.from) : "start"),
    ),
    ["Profile <- start", "format <- Profile"],
  );
});

test("crawl reaches a stopAt node but does not walk past it", async (root) => {
  const lookups = [routes(), callGraph("typescript").callees];
  const all = await crawl(lookups, profile(root));
  const runSql = [...all.keys()].find((n) => fnName(n) === "runSql");
  const names = [...(await crawl(lookups, profile(root), { stopAt: [runSql] })).keys()].map(fnName);
  equal([names.includes("runSql"), names.includes("audit")], [true, false]);
});

test("crawl takes lookups, not Maps, and says how to make one", async (root) => {
  const error = await throws(() => crawl(new Map(), profile(root)), "lookup(");
  equal(error instanceof TypeError, true);
});

test("join links two patterns where their captured names are equal, across files", (root) => {
  const [edge] = routes().get(profile(root));
  equal(
    [edge.name, edge.via, edge.site.text().startsWith("trpc.user.getUser")],
    ["getUser", "trpc", true],
  );
});

test("a crawl over a join and the callees reaches the procedure's handler and on", async (root) => {
  const name = (n) => fnName(n) ?? n.text().split(":")[0];
  const reached = await crawl([routes(), callGraph("typescript").callees], profile(root));
  equal([...reached.keys()].map(name), [
    "Profile",
    "getUser",
    "format",
    "() => loadUser()",
    "loadUser",
    "runSql",
    "audit",
  ]);
});

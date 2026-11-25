// join() links two patterns where their captured names are equal, across files.
import { callGraph, crawl, join } from "codemod:ast-grep";

const name = (n) => n.text().match(/function (\w+)/)?.[1] ?? n.text().split(":")[0];

export default async function transform(root) {
  const routes = join({
    language: "typescript",
    from: { find: "trpc.$R.$P.$M($$$)", anchor: "function", on: "$P" },
    to: { find: { rule: { pattern: { context: "({ $P: publicProcedure.$M($$$) })", selector: "pair" } } }, on: "$P" },
    meta: { via: "trpc" },
  });
  const profile = root.root().find("function Profile() { $$$ }");
  const [edge] = routes.get(profile);
  if (edge.name !== "getUser" || edge.via !== "trpc" || !edge.site.text().startsWith("trpc.user.getUser")) {
    throw new Error(JSON.stringify({ name: edge.name, via: edge.via, site: edge.site.text() }));
  }
  const { callees } = callGraph("typescript");
  const reached = [...(await crawl([routes, callees], profile)).keys()].map(name);
  // The procedure reaches its handler, then what the handler calls.
  const want = ["Profile", "getUser", "format", "() => loadUser()", "loadUser", "runSql", "audit"];
  if (JSON.stringify(reached) !== JSON.stringify(want)) throw new Error(JSON.stringify(reached));
}

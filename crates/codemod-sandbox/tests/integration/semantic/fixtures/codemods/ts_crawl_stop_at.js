// crawl() reaches a stopAt node but does not walk past it.
import { callGraph, crawl, join } from "codemod:ast-grep";

const name = (n) => n.text().match(/function (\w+)/)?.[1] ?? n.kind();

export default async function transform(root) {
  const { callees } = callGraph("typescript");
  const routes = join({
    language: "typescript",
    from: { find: "trpc.$R.$P.$M($$$)", anchor: "function", on: "$P" },
    to: { find: { rule: { pattern: { context: "({ $P: publicProcedure.$M($$$) })", selector: "pair" } } }, on: "$P" },
  });
  const start = root.root().find("function Profile() { $$$ }");
  const all = await crawl([routes, callees], start);
  const runSql = [...all.keys()].find((n) => name(n) === "runSql");
  const stopped = await crawl([routes, callees], start, { stopAt: [runSql] });
  const names = [...stopped.keys()].map(name);
  if (!names.includes("runSql") || names.includes("audit")) throw new Error(JSON.stringify(names));
}

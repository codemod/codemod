// crawl() takes lookups, not Maps, and says how to make one.
import { crawl } from "codemod:ast-grep";

export default async function transform(root) {
  const start = root.root().find("function Profile() { $$$ }");
  try {
    await crawl(new Map(), start);
  } catch (e) {
    if (e instanceof TypeError && e.message.includes("lookup(")) return;
    throw e;
  }
  throw new Error("a Map must be rejected");
}

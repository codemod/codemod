// A built-in lookup answers get(node) with the edges that leave it.
import { callGraph } from "codemod:ast-grep";

export default function transform(root) {
  const { callees, callers } = callGraph("typescript");
  const profile = root.root().find("function Profile() { $$$ }");
  const out = callees.get(profile).map((e) => e.node.text().split("\n")[0] + " @ " + e.site.text());
  if (JSON.stringify(out) !== JSON.stringify(["function format(value: unknown) { @ format(user)"])) {
    throw new Error("callees of Profile: " + JSON.stringify(out));
  }
  const format = root.root().find("function format($$$) { $$$ }");
  if (callers.get(format)[0].node.key() !== profile.key()) throw new Error("callers of format");
  if (callers.get(profile) !== undefined) throw new Error("Profile has no callers: undefined, like a Map");
  if (callers.has(profile)) throw new Error("has(): false without edges");
}

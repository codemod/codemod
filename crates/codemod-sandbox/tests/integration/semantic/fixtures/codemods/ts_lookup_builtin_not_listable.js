// A built-in lookup can't be listed: it is answered node by node.
import { callGraph } from "codemod:ast-grep";

export default function transform() {
  const { callers } = callGraph("typescript");
  for (const read of [() => callers.size, () => [...callers], () => callers.keys()]) {
    try {
      read();
    } catch (e) {
      if (!(e instanceof TypeError) || !String(e.message).includes("get(node)")) throw e;
      continue;
    }
    throw new Error("listing a built-in lookup must throw");
  }
}

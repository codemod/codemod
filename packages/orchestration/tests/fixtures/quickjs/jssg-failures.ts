// Parity: failing JSSG commands leave the repository untouched. The input
// picks the failure: a transform throwing on the second file, a cross-file
// rename conflict, or a rename escaping the target root.
import { dynamic } from "../../../src/index.ts";
import { conflict, escape, failSecond } from "../jssg/transforms.ts";

export default dynamic(async (which: string) => {
  if (which === "fail-second") return failSecond();
  if (which === "conflict") return conflict();
  return escape();
});

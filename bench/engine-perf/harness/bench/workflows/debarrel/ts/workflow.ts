// debarrel (registry 0.7.13) as a TS workflow: one jssg step with
// workspace-wide semantic analysis, same transform file.
import { jssg } from "@codemod.com/orchestration";
import transform from "../pkg/scripts/codemod.port.ts";

export default jssg({
  name: "debarrel",
  language: "tsx",
  semanticAnalysis: "workspace",
  transform,
});

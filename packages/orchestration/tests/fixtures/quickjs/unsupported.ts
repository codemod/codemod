// Fail-fast refusals in the quickjs runtime: the input picks the case.
// `agent` inside dynamic() is refused at the execution boundary; JSSG
// semantic analysis likewise.
import { agent, dynamic } from "../../../src/index.ts";
import { semantic } from "../jssg/transforms.ts";

const fix = agent({ name: "fix", prompt: "fix it", backend: { kind: "builtin" } });

export default dynamic(async (which: string) => (which === "agent" ? fix() : semantic()));

// Fail-fast refusal in the quickjs runtime: `agent` inside dynamic() is
// refused at the execution boundary.
import { agent, dynamic } from "../../../src/index.ts";

const fix = agent({ name: "fix", prompt: "fix it", backend: { kind: "builtin" } });

export default dynamic(async () => fix());

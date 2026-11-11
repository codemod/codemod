// A shell step, then a builtin agent step (through `codemod-agent`; the
// e2e test points CODEMOD_AGENT_BIN at a stand-in that echoes its launch).
import { agent, sequence, shell } from "../../../src/index.ts";

const before = shell({ name: "before", command: "touch ran.marker" });
const fix = agent({ name: "fix", prompt: "fix it", backend: { kind: "builtin" } });

export default sequence(before(), fix());

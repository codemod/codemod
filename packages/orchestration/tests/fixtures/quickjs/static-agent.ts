// A static agent step: refused before anything runs (static preflight).
import { agent, sequence, shell } from "../../../src/index.ts";

const before = shell({ name: "before", command: "touch ran.marker" });
const fix = agent({ name: "fix", prompt: "fix it", backend: { kind: "builtin" } });

export default sequence(before(), fix());

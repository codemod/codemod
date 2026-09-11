import { workflow } from "../../../src/index.ts";
import { migrate } from "./definitions.ts";

// Returns the artifact reference the build assigned next to the command's
// output, so a test can check both without the real bridge.
export default workflow(async () => {
  const output = await migrate({ target: { include: ["a.ts"] } });
  return { transform: migrate.transform, output };
});

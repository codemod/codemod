// Synthetic: ten sequential shell steps that do nothing (`true`). Measures per-step engine overhead.
import { sequence, shell } from "@codemod.com/orchestration";

const steps = Array.from({ length: 10 }, (_, i) => shell({ name: `step-${i + 1}`, command: "true" })());
export default sequence(...steps);

// Module-level state bundled into an artifact (unsupported in transforms;
// used to observe how often a runtime evaluates the artifact module).
let calls = 0;

export function nextCall(): number {
  calls += 1;
  return calls;
}

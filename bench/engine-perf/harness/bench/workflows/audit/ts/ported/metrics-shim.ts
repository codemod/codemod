// The node runtime refuses `codemod:metrics`, so the TS port collects each
// file's findings here and returns them as the step's typed `output`.
// Transforms run one at a time per JS thread, so draining after each file is safe.
export interface Finding { metric: string; dims: Record<string, string> }
const pending: Finding[] = [];
export function useMetricAtom(metric: string) {
  return {
    increment(dims: Record<string, string> = {}, by = 1) {
      for (let i = 0; i < by; i++) pending.push({ metric, dims });
    },
  };
}
export function drain(): Finding[] {
  return pending.splice(0);
}

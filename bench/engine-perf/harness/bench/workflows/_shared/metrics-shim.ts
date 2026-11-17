// Stand-in for `codemod:metrics`, which neither TS runtime serves today
// (node refuses the module; quickjs fails with "MetricsContext not found").
// Counts increments in memory so the transform's work stays the same.
const counts = new Map<string, number>();
export function useMetricAtom(name: string) {
  return {
    increment(_dims?: Record<string, string>, by = 1) {
      counts.set(name, (counts.get(name) ?? 0) + by);
    },
  };
}

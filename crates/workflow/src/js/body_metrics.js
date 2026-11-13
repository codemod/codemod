// `codemod/metrics` as the workflow body sees it. Declaring a metric at
// module level is fine (steps and callbacks get the real module); recording
// one in the body is not, because the body runs again on every resume.

function refuse() {
  throw new Error(
    "metrics can only be recorded inside a step or forEachFile callback; the workflow body runs again on every resume",
  );
}

export function useMetricAtom(name) {
  return Object.freeze({ name, increment: refuse, getEntries: () => [] });
}

export class MetricAtom {
  constructor() {
    refuse();
  }
}

export default { useMetricAtom, MetricAtom };

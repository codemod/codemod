// The data-flow and call-graph answers narrow the way a codemod uses them.
import type {
  CalleeEntry,
  CallerEntry,
  CallGraphEntries,
  FlowSteps,
  Mutation,
  SgNode,
  SgRoot,
} from "codemod:ast-grep";

declare const root: SgRoot;
declare const node: SgNode;

// Flow steps: `name` by kind, `index` by edge.
const steps: FlowSteps = node.flowsFrom({ throughCalls: false, maxDepth: 2 });
export const truncated: boolean = steps.truncated;
for (const step of steps) {
  if (step.kind === "parameter") {
    const name: string = step.name;
    const position: number = step.index;
    void [name, position];
  }
  if (step.kind === "call") {
    const callee: string = step.name;
    void callee;
  }
  if (step.via === "argument") {
    const position: number = step.index;
    void position;
  }
  if (step.kind === "literal") {
    const none: null = step.name;
    void none;
  }
  // @ts-expect-error a member's name is null when the property is computed
  const property: string = step.kind === "member" ? step.name : "";
  void property;
}
export const fromRequest: boolean = node
  .flowsFrom()
  .some((step) => step.kind === "parameter" && step.name === "req");
export const receivers: SgNode[] = node
  .flowsTo()
  .filter((step) => step.via === "argument")
  .map((step) => step.node);

// Callees: narrow on `resolved`.
const callees: CallGraphEntries<CalleeEntry> = node.callees({ transitive: true });
export const complete: boolean = !callees.truncated && callees.unparsed.length === 0;
for (const callee of callees) {
  const call: SgNode = callee.callSite.node;
  void call;
  if (callee.resolved) {
    // @ts-expect-error a resolved callee can be anonymous
    const name: string = callee.name;
    void name;
  } else {
    const name: string = callee.name;
    // @ts-expect-error an unresolved call is never a callback edge
    const via: "callback" = callee.via;
    void [name, via];
  }
}

// Callers: always resolved, always with a call site.
const callers: CallGraphEntries<CallerEntry> = node.callers();
for (const caller of callers) {
  const resolved: true = caller.resolved;
  const name: string = caller.name;
  const edit = caller.callSite.node.replace(`await ${caller.callSite.node.text()}`);
  void [resolved, name, edit];
}

// Mutations.
const mutations: Mutation[] = node.mutations({ after: root.root() });
export const definite = mutations.filter(
  (m) => m.confidence === "definite" && !m.viaAlias && !m.deferred,
);

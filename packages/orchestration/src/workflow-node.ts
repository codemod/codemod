/** Arbitrary TypeScript executed inside one orchestration run. */
export type Awaitable<T> = T | PromiseLike<T>;

export interface Workflow<I = unknown, O = unknown> {
  readonly type: "workflow";
  readonly body: (input: I) => Awaitable<O>;
}

export function workflow<O>(body: () => Awaitable<O>): Workflow<void, O>;
export function workflow<I, O>(body: (input: I) => Awaitable<O>): Workflow<I, O>;
export function workflow<I, O>(body: (input: I) => Awaitable<O>): Workflow<I, O> {
  return new WorkflowImpl(body);
}

export function isWorkflow(value: unknown): value is Workflow<never, unknown> {
  return value instanceof WorkflowImpl;
}

export function workflowRequiresInput(value: Workflow<never, unknown>): boolean {
  return value instanceof WorkflowImpl && value.requiresInput;
}

export async function runWorkflow<I, O>(definition: Workflow<I, O>, input: I): Promise<O> {
  return definition.body(input);
}

class WorkflowImpl<I, O> implements Workflow<I, O> {
  readonly type = "workflow" as const;
  readonly requiresInput: boolean;

  constructor(readonly body: (input: I) => Awaitable<O>) {
    this.requiresInput = body.length > 0;
  }
}

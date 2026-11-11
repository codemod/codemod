/** Arbitrary TypeScript executed inside one orchestration run. */
export type Awaitable<T> = T | PromiseLike<T>;

export interface Dynamic<I = unknown, O = unknown> {
  readonly type: "dynamic";
  readonly body: (input: I) => Awaitable<O>;
}

export function dynamic<O>(body: () => Awaitable<O>): Dynamic<void, O>;
export function dynamic<I, O>(body: (input: I) => Awaitable<O>): Dynamic<I, O>;
export function dynamic<I, O>(body: (input: I) => Awaitable<O>): Dynamic<I, O> {
  return new DynamicImpl(body);
}

export function isDynamic(value: unknown): value is Dynamic<never, unknown> {
  return value instanceof DynamicImpl;
}

export function dynamicRequiresInput(value: Dynamic<never, unknown>): boolean {
  return value instanceof DynamicImpl && value.requiresInput;
}

export async function runDynamic<I, O>(definition: Dynamic<I, O>, input: I): Promise<O> {
  return definition.body(input);
}

class DynamicImpl<I, O> implements Dynamic<I, O> {
  readonly type = "dynamic" as const;
  readonly requiresInput: boolean;

  constructor(readonly body: (input: I) => Awaitable<O>) {
    this.requiresInput = body.length > 0;
  }
}

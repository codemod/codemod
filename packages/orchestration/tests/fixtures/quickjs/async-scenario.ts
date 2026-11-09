/**
 * Async-context propagation scenario for the QuickJS implementation
 * (`src/host/quickjs/async-context.ts`, run by `async-context.ts`): every
 * labelled point must observe what Node's `AsyncLocalStorage` observes
 * (`crates/cli/tests/typescript_workflow.rs`). Labels are compared as a map,
 * not in order (lowered async takes a different number of microtask ticks).
 */
import type { AsyncContextVariable } from "../../../src/core/host.ts";

const tick = () =>
  Promise.resolve()
    .then(() => undefined)
    .then(() => undefined);

async function* numbers(): AsyncGenerator<number> {
  yield 1;
  await tick();
  yield 2;
}

export async function scenario(v: AsyncContextVariable<string>): Promise<Record<string, string>> {
  const seen: Record<string, string> = {};
  const record = (label: string) => {
    seen[label] = v.get() ?? "-";
  };
  record("top");
  let resolveShared!: () => void;
  const shared = new Promise<void>((resolve) => {
    resolveShared = resolve;
  });
  await v.run("A", async () => {
    record("A0");
    await null;
    record("A1");
    await Promise.all([
      v.run("B", async () => {
        record("B0");
        await Promise.resolve();
        record("B1");
        await tick();
        record("B2");
        await v.run("B.inner", async () => {
          await tick();
          record("Binner");
        });
        record("B3");
      }),
      v.run("C", async () => {
        record("C0");
        await tick();
        record("C1");
      }),
    ]);
    record("A2");
    await v.run("D", () => Promise.resolve().then(() => record("Dthen")));
    record("A3");
    // Created in A, resolved from outside any run, awaited inside E: the
    // continuation sees E (the context where it was registered).
    const waiter = v.run("E", async () => {
      await shared;
      record("E1");
    });
    resolveShared();
    await waiter;
    record("A4");
    await v.run("F", async () => {
      for await (const n of numbers()) record(`F${n}`);
      record("F3");
    });
    try {
      await v.run("G", async () => {
        await tick();
        throw new Error("boom");
      });
    } catch {
      record("Acatch");
    }
    await Promise.resolve()
      .finally(() => record("Afinally"))
      .then(() => record("Athen"));
    // Non-promise thenables (what an orchestration Command is): their `then`
    // must run in the frame where they were awaited, returned, or resolved.
    const thenable = (label: string): PromiseLike<string> => ({
      then<R1 = string, R2 = never>(resolve?: ((value: string) => R1 | PromiseLike<R1>) | null) {
        record(label);
        resolve?.(label);
        return undefined as unknown as PromiseLike<R1 | R2>;
      },
    });
    await v.run("H", async () => {
      await thenable("Hawait");
      await Promise.resolve(thenable("Hresolve"));
      await Promise.resolve().then(() => thenable("HfromThen"));
      await (async () => thenable("HfromAsync"))();
      await Promise.all([thenable("Hall")]);
      await new Promise((resolve) => resolve(thenable("Hexecutor")));
    });
  });
  record("end");
  return seen;
}

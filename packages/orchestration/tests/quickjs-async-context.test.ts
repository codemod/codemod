/**
 * The QuickJS async context (`src/host/quickjs/async-context.ts`) has
 * `AsyncLocalStorage` semantics once async code is lowered the way the
 * quickjs bundle lowers it. The scenario runs twice: natively in Node with
 * `AsyncLocalStorage`, and bundled with the quickjs build's lowering in a
 * fresh `node:vm` realm (its own `Promise`, so patching `then` there touches
 * nothing else). Every labelled observation must agree.
 */
import { AsyncLocalStorage } from "node:async_hooks";
import { resolve } from "node:path";
import vm from "node:vm";
import { build } from "esbuild";
import { expect, it } from "vitest";
import type { AsyncContextVariable } from "../src/core/host.ts";
import { scenario } from "./fixtures/quickjs/async-scenario.ts";

const src = resolve(import.meta.dirname, "../src");
const fixture = resolve(import.meta.dirname, "fixtures/quickjs/async-scenario.ts");

it("matches AsyncLocalStorage across awaits, Promise.all, nesting, finally, and async generators", async () => {
  const storage = new AsyncLocalStorage<string>();
  const reference: AsyncContextVariable<string> = {
    get: () => storage.getStore(),
    run: (value, fn) => storage.run(value, fn),
  };
  const expected = await scenario(reference);

  const entry = [
    `import { installPromiseContextPropagation, createPropagatingAsyncContext } from ${JSON.stringify(`${src}/host/quickjs/async-context.ts`)};`,
    `import { scenario } from ${JSON.stringify(fixture)};`,
    "installPromiseContextPropagation();",
    "globalThis.__run = () => scenario(createPropagatingAsyncContext());",
  ].join("\n");
  const result = await build({
    stdin: { contents: entry, resolveDir: src, loader: "ts" },
    bundle: true,
    write: false,
    format: "iife",
    platform: "neutral",
    target: "es2022",
    // The lowering crates/orchestration-build applies.
    supported: { "async-await": false, "async-generator": false, "for-await": false },
    logLevel: "silent",
  });
  const code = result.outputFiles[0]!.text;
  expect(code).not.toMatch(/\bawait\b/u);
  const realm = vm.createContext({});
  vm.runInContext(code, realm);
  const actual = (await (
    realm as { __run: () => Promise<Record<string, string>> }
  ).__run()) as Record<string, string>;

  expect(expected).toMatchObject({
    top: "-",
    A0: "A",
    B2: "B",
    Binner: "B.inner",
    E1: "E",
    end: "-",
  });
  expect({ ...actual }).toEqual(expected);
});

it("without lowering, native await loses the context (why the bundle lowers async)", async () => {
  const entry = [
    `import { installPromiseContextPropagation, createPropagatingAsyncContext } from ${JSON.stringify(`${src}/host/quickjs/async-context.ts`)};`,
    "installPromiseContextPropagation();",
    "const v = createPropagatingAsyncContext();",
    "globalThis.__run = () => v.run('A', async () => { await null; return v.get() ?? '-'; });",
  ].join("\n");
  const result = await build({
    stdin: { contents: entry, resolveDir: src, loader: "ts" },
    bundle: true,
    write: false,
    format: "iife",
    platform: "neutral",
    target: "es2022",
    logLevel: "silent",
  });
  const realm = vm.createContext({});
  vm.runInContext(result.outputFiles[0]!.text, realm);
  expect(await (realm as { __run: () => Promise<string> }).__run()).toBe("-");
});

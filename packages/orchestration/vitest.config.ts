import { configDefaults, defineConfig } from "vitest/config";

const e2e =
  process.env.CODEMOD_BRIDGE_E2E === "1" ||
  process.env.CODEMOD_QUICKJS_E2E === "1" ||
  process.env.CODEMOD_TIMINGS_E2E === "1";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    exclude: e2e ? configDefaults.exclude : [...configDefaults.exclude, "**/*.e2e.test.ts"],
    testTimeout: 60_000,
    // The addon's JS keeps per-process transform state and evaluates
    // artifacts through Node's own module loader; it must be one native
    // module instance, never a vite-transformed copy.
    server: { deps: { external: [/jssg-napi/] } },
  },
});

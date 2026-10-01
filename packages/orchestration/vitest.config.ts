import { configDefaults, defineConfig } from "vitest/config";

const e2e = process.env.CODEMOD_BRIDGE_E2E === "1";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    exclude: e2e ? configDefaults.exclude : [...configDefaults.exclude, "**/*.e2e.test.ts"],
    testTimeout: 60_000,
  },
});

import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    // Vitest runs on Node, so the specs install a Node host; the runtime itself
    // is the QuickJS host in the codemod binary.
    setupFiles: ["tests/support/node-host.ts"],
    testTimeout: 60_000,
  },
});

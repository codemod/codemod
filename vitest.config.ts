import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    setupFiles: [],
    env: {
      NODE_ENV: "test",
      CODEMOD_COM_API_URL: "https://codemod.com/api",
    },
    exclude: [...configDefaults.exclude, "./packages/deprecated/**"],
    include: [...configDefaults.include, "**/test/*.ts"],
    passWithNoTests: true,
    testTimeout: 15_000,
  },
});

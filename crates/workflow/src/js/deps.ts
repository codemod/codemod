// The `codemod/deps` module of the engine's QuickJS context: `install` from
// the orchestrator, and `dependencies`, `registry` and `semver` in the
// workflow body. Inside steps and callbacks `codemod/deps` is
// `dependencies.ts`, which runs in the isolate.

import { makeSemver } from "codemod-internal:semver";
import { __install } from "codemod/workflow";

export const install = __install;

/** Reads the repository's dependencies; only inside a step, so what it reads is recorded. */
export function dependencies(): never {
  throw new Error(
    "dependencies() must run inside a step, so what it reads is recorded: await step(() => dependencies()...)",
  );
}

/** Removed; kept so a file that still imports it fails with what to write instead. */
export function dependencyGraph(): never {
  throw new Error(
    'dependencyGraph() was removed: call dependencies() from "codemod/deps" inside a step, as step(() => { const deps = dependencies(); ... }); `codemod workflow upgrade` rewrites it',
  );
}

/** Package releases from npm, crates.io and PyPI; only inside a step or callback, so the answer is recorded. */
export const registry = Object.freeze({
  npm: () => outsideStep("npm"),
  cargo: () => outsideStep("cargo"),
  pypi: () => outsideStep("pypi"),
});

function outsideStep(ecosystem: string): Promise<never> {
  return Promise.reject(
    new Error(`registry.${ecosystem}(...) must run inside a step, so its answer is recorded`),
  );
}

/** Versions and ranges, by npm rules, Cargo's (`{ ecosystem: "cargo" }`) or PEP 440 (`{ ecosystem: "pypi" }`). */
export const semver = makeSemver((op, ecosystem, a, b) =>
  JSON.parse(globalThis.__workflowHost.semver(op, ecosystem, a, b)),
);

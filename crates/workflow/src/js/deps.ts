// The `codemod/deps` module of the engine's QuickJS context: `dependencyGraph`
// from the orchestrator, and `registry` and `semver` in the workflow body.
// Inside steps and callbacks `codemod/deps` is `dependencies.ts`, which runs
// in the isolate.

import { makeSemver } from "codemod-internal:semver";
import { __dependencyGraph } from "codemod/workflow";

export const dependencyGraph = __dependencyGraph;

/** Package releases from npm, crates.io, PyPI and Packagist; only inside a step or callback, so the answer is recorded. */
export const registry = Object.freeze({
  npm: () => outsideStep("npm"),
  cargo: () => outsideStep("cargo"),
  pypi: () => outsideStep("pypi"),
  packagist: () => outsideStep("packagist"),
});

function outsideStep(ecosystem: string): Promise<never> {
  return Promise.reject(
    new Error(`registry.${ecosystem}(...) must run inside a step, so its answer is recorded`),
  );
}

/** Versions and ranges, by npm rules, Cargo's (`{ ecosystem: "cargo" }`), PEP 440 (`{ ecosystem: "pypi" }`) or Composer's (`{ ecosystem: "packagist" }`). */
export const semver = makeSemver((op, ecosystem, a, b) =>
  JSON.parse(globalThis.__workflowHost.semver(op, ecosystem, a, b)),
);

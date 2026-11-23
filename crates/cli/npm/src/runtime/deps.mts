// `codemod:deps` in Node.js and Bun. The dependency APIs need the engine's
// lockfile reader and registry client, which only the codemod CLI has. They
// are exported here so a workflow that imports them still loads, and fail
// with a clear message when used.

const QUICKJS_ONLY =
  'is not available on the Node.js host yet: run the workflow with the codemod CLI, or test it with { host: "quickjs" }';

function unavailable(what: string): never {
  throw new Error(`${what} ${QUICKJS_ONLY}`);
}

export function dependencyGraph(..._args: unknown[]): never {
  return unavailable("dependencyGraph()");
}

export const registry = Object.freeze({
  npm: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.npm() ${QUICKJS_ONLY}`)),
  cargo: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.cargo() ${QUICKJS_ONLY}`)),
  pypi: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.pypi() ${QUICKJS_ONLY}`)),
});

export const semver = Object.freeze({
  satisfies: (): never => unavailable("semver.satisfies()"),
  compare: (): never => unavailable("semver.compare()"),
  gt: (): never => unavailable("semver.gt()"),
  lt: (): never => unavailable("semver.lt()"),
  isBreaking: (): never => unavailable("semver.isBreaking()"),
  isPrerelease: (): never => unavailable("semver.isPrerelease()"),
});

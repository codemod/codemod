// `codemod:deps` in Node.js and Bun. Reading dependencies needs the engine's
// manifest and lockfile readers and registry client, which only the codemod
// CLI has. They are exported here so a workflow that imports them still
// loads, and fail with a clear message when used. `install()` is a step the
// engine runs, so it works on both hosts.

export { __install as install } from "./workflow.mts";

const QUICKJS_ONLY =
  'is not available on the Node.js host yet: run the workflow with the codemod CLI, or test it with { host: "quickjs" }';

function unavailable(what: string): never {
  throw new Error(`${what} ${QUICKJS_ONLY}`);
}

export function dependencies(..._args: unknown[]): never {
  return unavailable("dependencies()");
}

export const registry = Object.freeze({
  npm: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.npm() ${QUICKJS_ONLY}`)),
  cargo: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.cargo() ${QUICKJS_ONLY}`)),
  pypi: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.pypi() ${QUICKJS_ONLY}`)),
});

const semverNames = [
  "satisfies",
  "compare",
  "gt",
  "gte",
  "lt",
  "lte",
  "isBreaking",
  "isPrerelease",
  "minVersion",
  "major",
  "coerce",
  "maxSatisfying",
  "validRange",
] as const;

export const semver = Object.freeze(
  Object.fromEntries(
    semverNames.map((name) => [name, (): never => unavailable(`semver.${name}()`)]),
  ) as Record<(typeof semverNames)[number], () => never>,
);

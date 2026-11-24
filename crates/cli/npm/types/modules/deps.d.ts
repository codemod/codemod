// `codemod/deps`: the repository's dependencies from its lockfiles, package
// releases from the registries, and version comparison.

declare module "codemod:deps" {
  import type { Globs, Task } from "codemod:workflow";

  export type Ecosystem = "npm" | "cargo" | "pypi";
  export type DependencyKind = "prod" | "dev" | "peer" | "optional" | "build";
  export type KindsOption = { kinds?: DependencyKind | readonly DependencyKind[] };
  /**
   * Options of the methods that list dependents. They list the repository's
   * own packages only, unless `includeThirdParty` is true.
   */
  export type DependentsOptions = KindsOption & { includeThirdParty?: boolean };

  /** A `name@version` the lockfiles install, or one of the repository's own packages. */
  export interface PackageNode {
    readonly ecosystem: Ecosystem;
    readonly name: string;
    readonly version: string;
    /** Directory of one of the repository's own packages, relative to the target; null for installed packages. */
    readonly dir: string | null;
    /** Whether this is one of the repository's own packages (`dir` is set). */
    readonly local: boolean;
    /** Edges this node declares. */
    dependencies(options?: KindsOption): DependencyEdge[];
    /**
     * Edges installed onto this exact node, from the repository's own
     * packages unless `includeThirdParty`. `g.dependents(name)` also has
     * declarations the lockfile does not resolve.
     */
    installedBy(options?: DependentsOptions): DependencyEdge[];
    /**
     * This package's releases from the registry, oldest first, each with
     * `breaking` relative to this node's version. Yanked releases are dropped.
     */
    releases(options?: {
      /** `{ react: "19.0.0" }` keeps releases whose ranges on react accept 19.0.0. Not for pypi packages. */
      accepting?: Record<string, string>;
      newerThanMine?: boolean;
      prereleases?: boolean;
    }): Promise<(Release & { breaking: boolean })[]>;
    /** `name@version`, which is also what the node becomes in a step's result. */
    toString(): string;
  }

  /** One declared dependency. */
  export interface DependencyEdge {
    readonly from: PackageNode;
    /** The installed node; null when the lockfile does not resolve it, as for most peer edges. */
    readonly to: PackageNode | null;
    readonly name: string;
    /** Declared range; null when the lockfile keeps none (third-party crates in Cargo.lock). */
    readonly range: string | null;
    readonly kind: DependencyKind;
    /** Whether the declared range accepts `version`; null when it cannot tell. */
    accepts(version: string): boolean | null;
  }

  /** What an edge becomes in a step's result. */
  export interface RecordedEdge {
    from: string;
    /** `name@version`, or the bare name when the edge does not resolve. */
    to: string;
    kind: DependencyKind;
    range: string | null;
  }

  /** A `dependencyGraph` result as recorded: nodes become `name@version`, edges `RecordedEdge`. */
  export type Recorded<T> = T extends PackageNode
    ? string
    : T extends DependencyEdge
      ? RecordedEdge
      : T extends (...args: never[]) => unknown
        ? T
        : T extends readonly (infer U)[]
          ? Recorded<U>[]
          : T extends object
            ? { [K in keyof T]: Recorded<T[K]> }
            : T;

  /**
   * The repository's lockfiles as a live graph. It is only valid inside the
   * `dependencyGraph` callback; only what the callback returns is recorded.
   */
  export interface DependencyGraph {
    /**
     * Lockfiles read, relative to the target. Empty when the target has no
     * lockfile at its root, or none that can be read: the graph is empty.
     */
    readonly lockfiles: readonly string[];
    /**
     * Lockfiles found but not read, with why. Without a `lockfiles` option,
     * a root lockfile that cannot be read is left out instead of failing the
     * step.
     */
    readonly skipped: readonly { readonly lockfile: string; readonly error: string }[];
    /** The repository's own packages. */
    workspaces(): PackageNode[];
    /** Every node, or every installed version of `name`. */
    nodes(name?: string): PackageNode[];
    /**
     * One node by name, or by name and version. Throws when several versions
     * are installed and no version is given: use `nodes(name)` then.
     */
    node(name: string, version?: string): PackageNode | undefined;
    /**
     * Edges that declare a dependency on `name`, resolved or not (a peer edge
     * resolves only when pnpm records the installed peer), from the
     * repository's own packages unless `includeThirdParty`.
     */
    dependents(name: string, options?: DependentsOptions): DependencyEdge[];
    /**
     * The shortest chain from each `from` node (default: every one of the repository's own packages) to
     * any `to` node, `from` first and the reached node last. Chains pass
     * through the repository's own packages, so web reaches moment through
     * `web > @demo/shared > moment`. A `from` that reaches none has no chain.
     */
    chains(options: {
      from?: PackageNode | readonly PackageNode[];
      to: PackageNode | readonly PackageNode[];
      kinds?: DependencyKind | readonly DependencyKind[];
    }): [from: PackageNode, ...rest: PackageNode[]][];
  }

  /**
   * Reads the target's lockfiles (yarn.lock, pnpm-lock.yaml, package-lock.json,
   * npm-shrinkwrap.json, bun.lock, Cargo.lock, uv.lock, poetry.lock,
   * Pipfile.lock; default: those at the target's root, so nested ones need a
   * `lockfiles` glob, which fails the step when it matches none) into a graph and runs `callback` on it in
   * its own isolate, where it may await `registry` lookups. Nodes and edges in
   * the result are recorded as `name@version` text and `RecordedEdge`.
   * Write `callback` inline, as an arrow or function expression: the build
   * extracts it, and a callback passed by name is a build error.
   *
   * @example
   * ```ts
   * const muiUsers = await step(dependencyGraph((g) =>
   *   g.dependents("@material-ui/core", { kinds: "prod" }).map((edge) => edge.from.dir),
   * ));
   * ```
   */
  export function dependencyGraph<Output>(
    callback: (graph: DependencyGraph) => Output | PromiseLike<Output>,
  ): Task<Recorded<Awaited<Output>>>;
  export function dependencyGraph<Output>(
    options: { lockfiles?: Globs },
    callback: (graph: DependencyGraph) => Output | PromiseLike<Output>,
  ): Task<Recorded<Awaited<Output>>>;

  export interface Release {
    version: string;
    yanked: boolean;
    prerelease: boolean;
    dependencies: Record<string, string>;
    peerDependencies: Record<string, string>;
    optionalDependencies: Record<string, string>;
    /** cargo only. */
    buildDependencies?: Record<string, string>;
    /** cargo only. */
    devDependencies?: Record<string, string>;
  }

  export interface PackageReleases {
    ecosystem: Ecosystem;
    name: string;
    /** Oldest first. Empty when the registry does not know the package. */
    versions: Release[];
  }

  /**
   * Release metadata from npm, crates.io and PyPI. Usable inside a function step or
   * a `forEachFile` / `dependencyGraph` callback, so the answer is recorded
   * with the step; in the workflow body it rejects. Lookups are cached for the
   * run and asked once per package.
   */
  export const registry: {
    npm(name: string): Promise<PackageReleases>;
    cargo(name: string): Promise<PackageReleases>;
    /** Releases carry no dependencies: PyPI lists them per file, not per release. */
    pypi(name: string): Promise<PackageReleases>;
  };

  type EcosystemOption = { ecosystem?: Ecosystem };
  /** Version comparison with npm rules, Cargo's with `{ ecosystem: "cargo" }`, or PEP 440 with `{ ecosystem: "pypi" }`. */
  export const semver: {
    /** true, false, or null when the range cannot be read (`workspace:`, `git:` ...). */
    satisfies(version: string, range: string, options?: EcosystemOption): boolean | null;
    compare(a: string, b: string, options?: EcosystemOption): -1 | 0 | 1;
    gt(a: string, b: string, options?: EcosystemOption): boolean;
    lt(a: string, b: string, options?: EcosystemOption): boolean;
    /** Whether `from` to `to` changes the left-most non-zero component (0.29 to 0.30 is breaking). */
    isBreaking(from: string, to: string, options?: EcosystemOption): boolean;
    isPrerelease(version: string, options?: EcosystemOption): boolean;
  };
}

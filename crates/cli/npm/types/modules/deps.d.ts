// `codemod/deps`: the repository's dependencies (its package.json files, and
// its lockfiles when asked), edits to them, the package manager, package
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
  export type PackageManagerName = "npm" | "pnpm" | "yarn" | "bun";

  /** One of the repository's packages, its directory ("." for the root), or its name. */
  export type PackageRef = Package | string;
  /** Which packages an edit or a lookup covers: kinds, and packages (default: all). */
  export type EditScope = KindsOption & { in?: PackageRef | readonly PackageRef[] };
  /** A new range, or a function of the current declaration (undefined leaves it). */
  export type RangeOrUpdate = string | ((current: Declaration) => string | undefined);

  export interface PackageManager {
    readonly name: PackageManagerName;
    /** From `packageManager` in the root package.json ("pnpm@9.12.0" gives "9.12.0"); null without it. */
    readonly version: string | null;
    /** The root lockfile it keeps, relative to the target; null when there is none. */
    readonly lockfile: string | null;
    /** What told: the packageManager field, a lockfile, pnpm-workspace.yaml, or nothing (npm). */
    readonly detectedFrom: "packageManager" | "lockfile" | "pnpm-workspace.yaml" | "default";
  }

  /** One of the repository's own packages, read from its package.json. */
  export interface Package {
    readonly ecosystem: "npm";
    /** `name` from package.json; null when it has none. */
    readonly name: string | null;
    readonly version: string | null;
    /** Directory relative to the target: "." for the root. */
    readonly dir: string;
    /** Its package.json, relative to the target. */
    readonly manifest: string;
    /**
     * Whether it is one of the repository's packages: the root, or a
     * workspace the root declares. Only `packages({ all: true })` and
     * `packageOf(file, { all: true })` give packages that are not.
     */
    readonly member: boolean;
    /** Every declaration, in manifest order (dependencies, devDependencies, peerDependencies, optionalDependencies). */
    declarations(options?: KindsOption): Declaration[];
    /** The declaration of `name`: a runtime one (prod, optional, peer) before dev. */
    declaration(name: string, options?: KindsOption): Declaration | undefined;
    uses(name: string, options?: KindsOption): boolean;
    /** `dependencies().add(name, range, { to: this, kind })`. */
    add(name: string, range: string, options?: { kind?: DependencyKind }): EditResult;
    /** `dependencies().bump(name, range, { in: this, kinds })`. */
    bump(name: string, range: RangeOrUpdate, options?: KindsOption): EditResult;
    remove(name: string, options?: KindsOption): EditResult;
    rename(from: string, to: string, options?: KindsOption & { range?: string }): EditResult;
    /** `name@version`, the name, or the dir when it has no name: what it becomes in a step's result. */
    toString(): string;
    toJSON(): string;
  }

  export type Protocol = "registry" | "catalog" | "workspace" | "alias" | "other";

  /** One dependency as a package.json declares it. */
  export interface Declaration {
    readonly package: Package;
    readonly name: string;
    /** As written: "^18.2.0", "catalog:", "workspace:^", "npm:other@^1", "github:a/b". */
    readonly range: string;
    readonly kind: DependencyKind;
    /** "catalog" and "workspace" refs, "alias" (npm:), "registry" ranges, or "other" (git, file, url, tags). */
    readonly protocol: Protocol;
    /**
     * The range it means: a catalog ref gives its catalog entry, a workspace
     * ref the local package's version ("workspace:^" with 1.2.0 gives
     * "^1.2.0"), an alias its range; null when it cannot be resolved.
     */
    readonly resolved: string | null;
    /** The catalog a catalog ref points to; null otherwise. */
    readonly catalog: { readonly name: string; readonly file: string } | null;
    /** The package a workspace ref points to; null otherwise. */
    readonly local: Package | null;
    /** Whether `resolved` is a range `semver` can read (not git, file, a url, or a tag such as "latest"). */
    readonly readable: boolean;
    /** Whether `resolved` accepts `version`: true, false, or null when it cannot tell. */
    accepts(version: string): boolean | null;
    /** The lowest version `resolved` accepts ("^18.2.0" gives "18.2.0"); null when unreadable. */
    minVersion(): string | null;
    /** What the lockfile installs for it (loads the graph); null when it does not resolve it. */
    installed(): PackageNode | null;
    toJSON(): RecordedDeclaration;
  }

  /** What a declaration becomes in a step's result. */
  export interface RecordedDeclaration {
    package: string;
    name: string;
    range: string;
    kind: DependencyKind;
    resolved: string | null;
  }

  /** What an edit changed. */
  /** A declaration an edit left alone. */
  export interface SkippedEdit {
    /** Its package.json. */
    readonly file: string;
    /** Its package's dir. */
    readonly package: string;
    readonly name: string;
    readonly kind: DependencyKind;
    readonly range: string;
    /**
     * "peer": a peer dependency, which edits change only when `kinds` names
     * "peer"; "workspace": a `workspace:` ref, which points at one of the
     * repository's own packages and is not bumped, renamed, or overwritten by `add`.
     */
    readonly reason: "peer" | "workspace";
  }

  /**
   * What an edit changed, as an array of `Edit`s, with what it left alone as
   * `skipped`. In a step's result it is recorded as its edits (`toJSON`):
   * return `result.skipped` to keep that too.
   */
  export type EditResult = readonly Edit[] & {
    readonly skipped: readonly SkippedEdit[];
    toJSON(): Edit[];
  };

  export interface Edit {
    /** The file: a package.json, pnpm-workspace.yaml or .yarnrc.yml. */
    readonly file: string;
    /** The package's dir; null for a catalog entry. */
    readonly package: string | null;
    readonly name: string;
    /** The section, or "catalog" for a catalog entry. */
    readonly kind: DependencyKind | "catalog";
    /** The range before; null when added. */
    readonly from: string | null;
    /** The range after; null when removed. */
    readonly to: string | null;
    /** For a rename: the old name. */
    readonly renamedFrom?: string;
  }

  export interface Catalog {
    /** "default" for `catalog:`. */
    readonly name: string;
    readonly file: string;
    readonly entries: Readonly<Record<string, string>>;
  }

  interface NodeBase {
    readonly ecosystem: Ecosystem;
    readonly name: string;
    readonly version: string;
    /** Edges this node declares. */
    dependencies(options?: KindsOption): DependencyEdge[];
    /**
     * Edges installed onto this exact node, from the repository's own
     * packages unless `includeThirdParty`. `dependents(name)` also has
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
    toJSON(): string;
  }

  /** One of the repository's own packages, as the lockfile has it. */
  export interface LocalNode extends NodeBase {
    readonly local: true;
    /** Its directory, relative to the target. */
    readonly dir: string;
  }

  /** A `name@version` the lockfiles install. */
  export interface InstalledNode extends NodeBase {
    readonly local: false;
    readonly dir: null;
  }

  /** A node of the lockfile graph: `local` tells which, and narrows `dir`. */
  export type PackageNode = LocalNode | InstalledNode;

  /** One declared dependency, in the lockfile graph. */
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
    toJSON(): RecordedEdge;
  }

  /** What an edge becomes in a step's result. */
  export interface RecordedEdge {
    from: string;
    /** `name@version`, or the bare name when the edge does not resolve. */
    to: string;
    kind: DependencyKind;
    range: string | null;
  }

  /** A node, one of the repository's packages, or a package name (every installed version of it). */
  export type NodeRef = PackageNode | Package | string;

  /**
   * The repository's dependencies. Manifest members read package.json files
   * (no lockfile needed). Graph members load the lockfiles the first time one
   * is used, and keep what they loaded: an edit changes the manifests, not
   * what is installed (see `needsInstall`).
   */
  export interface Dependencies {
    /** The repository's package manager; null without a root package.json. */
    readonly packageManager: PackageManager | null;
    /**
     * The repository's packages: the root, then the workspaces it declares
     * (`workspaces`, or pnpm-workspace.yaml's `packages`), by dir. Without a
     * workspace declaration, only the root. `{ all: true }`: every
     * package.json under the target (`.gitignore` respected, `node_modules` skipped).
     */
    packages(options?: { all?: boolean }): Package[];
    /** A package by dir or by name, the repository's packages first; any package.json counts. */
    package(ref: string): Package | undefined;
    /**
     * The package that owns a file: the nearest of the repository's packages
     * at or above it; with `{ all: true }`, the nearest package.json.
     */
    packageOf(file: string, options?: { all?: boolean }): Package | undefined;
    /** Whether any of the repository's packages declares `name`. */
    uses(name: string, options?: KindsOption): boolean;
    /** Every declaration of `name` across the packages (or those `in`), by package dir. */
    declarations(name: string, options?: EditScope): Declaration[];
    /** Catalogs: pnpm-workspace.yaml, bun's package.json, yarn's .yarnrc.yml. */
    catalogs(): Catalog[];

    /**
     * Set the range of `name` everywhere it is declared (or `in`, of
     * `kinds`). Peer dependencies change only when `kinds` names "peer". A
     * catalog ref edits its catalog entry once; a workspace ref is left
     * alone; an npm: alias keeps its target. Peers and workspace refs it
     * leaves are in the result's `skipped`. The edits are the step's file
     * changes: shown in a dry run, written when the step ends.
     */
    bump(name: string, range: RangeOrUpdate, options?: EditScope): EditResult;
    /** Declare `name` in a package (default: the root) and section (default: prod); already there, its range is set. */
    add(
      name: string,
      range: string,
      options?: { to?: PackageRef; kind?: DependencyKind },
    ): EditResult;
    /** Remove `name` from every section that declares it (or those in `kinds`, `in`); peers only when `kinds` names "peer". */
    remove(name: string, options?: EditScope): EditResult;
    /**
     * Rename `from` to `to` where declared, keeping the section, and the
     * range unless `range` is given; peers only when `kinds` names "peer",
     * and never a workspace ref.
     */
    rename(from: string, to: string, options?: EditScope & { range?: string }): EditResult;
    /** Every edit made through this object, in order. */
    readonly edits: readonly Edit[];
    /**
     * Whether the lockfile is behind the manifests (loads the graph): then run
     * `await step(install())`. yarn.lock keeps no workspace ranges, so on
     * Yarn a changed range shows only in the step that changed it; added and
     * removed dependencies show in later steps too.
     */
    needsInstall(): boolean;

    /** Lockfiles read, relative to the target; empty when there is none. Loads the graph. */
    readonly lockfiles: readonly string[];
    /** Lockfiles found but not read, with why. */
    readonly skipped: readonly { readonly lockfile: string; readonly error: string }[];
    /** Installed versions of `name`, lowest first. */
    installed(name: string): string[];
    /** The repository's own packages as the lockfiles have them (Cargo and Python ones too). */
    workspaces(): LocalNode[];
    /** Every node, or every installed version of `name`. */
    nodes(name?: string): PackageNode[];
    /** One node by name, or by name and version. Throws when several versions are installed and no version is given. */
    node(name: string, version?: string): PackageNode | undefined;
    /**
     * Edges that declare a dependency on `name`, resolved or not, from the
     * repository's own packages unless `includeThirdParty`.
     */
    dependents(name: string, options?: DependentsOptions): DependencyEdge[];
    /**
     * The shortest chain from each `from` (default: every one of the
     * repository's own packages) to any `to`, `from` first. A name means every
     * installed version of it; one that is not installed reaches nothing.
     * Chains pass through the repository's own packages.
     *
     * @example
     * ```ts
     * deps.chains({ to: "moment" }).map((chain) => chain.join(" > "))
     * // ["@demo/web@1.0.0 > @demo/shared@1.0.0 > moment@2.29.4", ...]
     * ```
     */
    chains(options: {
      from?: NodeRef | readonly NodeRef[];
      to: NodeRef | readonly NodeRef[];
      kinds?: DependencyKind | readonly DependencyKind[];
    }): [from: PackageNode, ...rest: PackageNode[]][];
  }

  /**
   * The repository's dependencies, inside a step (a function step, a
   * `forEachFile` callback, or a file they import). Reads its package.json
   * files now; its lockfiles (the root ones, or those `lockfiles` matches)
   * when a graph member is first used. One object per step and options.
   * In the workflow body it throws: read in a step, so the answer is recorded.
   *
   * @example
   * ```ts
   * const users = await step(() => dependencies().declarations("moment").map((d) => d.package.dir));
   * await step(() => dependencies().bump("react", "^19.0.0"));
   * ```
   */
  export function dependencies(options?: { lockfiles?: Globs }): Dependencies;

  export interface InstallOptions {
    /** Package directory to run in, relative to the target. Default: the root. */
    dir?: string;
    /**
     * Only refresh the lockfile: npm --package-lock-only, pnpm --lockfile-only,
     * yarn --mode=update-lockfile, bun --lockfile-only. Yarn 1 has no such mode
     * and installs. Default: true.
     */
    lockfileOnly?: boolean;
  }

  export interface InstallResult {
    readonly packageManager: PackageManagerName;
    readonly command: string;
    /** False in a dry run, which does not run it. */
    readonly ran: boolean;
    readonly lockfile: string | null;
    /** Whether the lockfile's text changed. */
    readonly changed: boolean;
  }

  /**
   * Runs the repository's package manager so the lockfile catches up with
   * the manifests: a step, in the workflow body. A dry run does not run it.
   * A package manager that is not installed, or a registry that cannot be
   * reached, fails the step with a message instead of hanging.
   *
   * @example
   * ```ts
   * await step(() => dependencies().bump("react", "^19.0.0"));
   * await step(install());
   * ```
   */
  export function install(options?: InstallOptions): Task<InstallResult>;

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
   * a `forEachFile` callback, so the answer is recorded with the step; in the
   * workflow body it rejects. Lookups are cached for the run and asked once per package.
   */
  export const registry: {
    npm(name: string): Promise<PackageReleases>;
    cargo(name: string): Promise<PackageReleases>;
    /** Releases carry no dependencies: PyPI lists them per file, not per release. */
    pypi(name: string): Promise<PackageReleases>;
  };

  export type EcosystemOption = { ecosystem?: Ecosystem };
  /** Version comparison with npm rules, Cargo's with `{ ecosystem: "cargo" }`, or PEP 440 with `{ ecosystem: "pypi" }`. */
  export const semver: {
    /** true, false, or null when the range cannot be read (`workspace:`, `git:` ...). */
    satisfies(version: string, range: string, options?: EcosystemOption): boolean | null;
    compare(a: string, b: string, options?: EcosystemOption): -1 | 0 | 1;
    gt(a: string, b: string, options?: EcosystemOption): boolean;
    gte(a: string, b: string, options?: EcosystemOption): boolean;
    lt(a: string, b: string, options?: EcosystemOption): boolean;
    lte(a: string, b: string, options?: EcosystemOption): boolean;
    /** Whether `from` to `to` changes the left-most non-zero component (0.29 to 0.30 is breaking). */
    isBreaking(from: string, to: string, options?: EcosystemOption): boolean;
    isPrerelease(version: string, options?: EcosystemOption): boolean;
    /** The lowest version a range accepts: "^18.2.0" gives "18.2.0"; null when it cannot be read. */
    minVersion(range: string, options?: EcosystemOption): string | null;
    /** The major of a version, or of a range's lowest version: major("^18.2.0") is 18; null when unreadable. */
    major(versionOrRange: string, options?: EcosystemOption): number | null;
    /** The first version in a text, padded to three parts: "v1.2" gives "1.2.0", "pnpm@10.1.3" "10.1.3"; null when none. */
    coerce(text: string): string | null;
    /** The highest of `versions` the range accepts; null when none does. */
    maxSatisfying(
      versions: readonly string[],
      range: string,
      options?: EcosystemOption,
    ): string | null;
    /** Whether a range can be read (not git, file, a url, or a tag such as "latest"). */
    validRange(range: string, options?: EcosystemOption): boolean;
  };
}

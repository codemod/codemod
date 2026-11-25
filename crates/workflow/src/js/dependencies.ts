// `codemod/deps` inside isolates (function steps, forEachFile callbacks,
// and the files they import): `dependencies()`, `registry` and `semver`.
// The host installs `globalThis.__dependencyHost` in every isolate
// (crates/workflow/src/deps_host.rs, `globals.d.ts`).

import { makeSemver } from "codemod-internal:semver";
import type { Ecosystem } from "codemod-internal:semver";

/** One release as the host reports it (crates/deps `Release`). */
interface Release {
  version: string;
  yanked: boolean;
  prerelease: boolean;
  [group: string]: unknown;
}

interface Package {
  versions: Release[];
}

type Answer = { ok: true; found: Record<string, Package> } | { ok: false; error: string };

interface Waiting {
  resolve(found: Package): void;
  reject(error: Error): void;
}

const host = () => globalThis.__dependencyHost;

function freeze<T>(value: T): T {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const item of Object.values(value)) freeze(item);
  }
  return value;
}

// Lookups started in the same turn go to the host as one batch per
// ecosystem, so `Promise.all` over many packages fetches them together.
// Each package is asked once per isolate; the host caches across the run.
const answers = new Map<string, Promise<Package>>();
let queued: Map<Ecosystem, Map<string, Waiting>> | null = null;

function lookup(ecosystem: Ecosystem, name: unknown): Promise<Package> {
  if (typeof name !== "string" || name === "") {
    return Promise.reject(new TypeError(`registry.${ecosystem}(name) expects a package name`));
  }
  const key = `${ecosystem}:${name}`;
  let answer = answers.get(key);
  if (!answer) {
    answer = new Promise<Package>((resolve, reject) => {
      if (!queued) {
        queued = new Map();
        void Promise.resolve().then(flush);
      }
      const names = queued.get(ecosystem) ?? new Map<string, Waiting>();
      names.set(name, { resolve, reject });
      queued.set(ecosystem, names);
    });
    answers.set(key, answer);
  }
  return answer;
}

function flush(): void {
  const batch = queued;
  queued = null;
  if (!batch) return;
  for (const [ecosystem, names] of batch) {
    void host()
      .releases(ecosystem, JSON.stringify([...names.keys()]))
      .then((json) => JSON.parse(json) as Answer)
      .catch(
        (error: unknown): Answer => ({
          ok: false,
          error: error instanceof Error ? error.message : String(error),
        }),
      )
      .then((answer) => {
        for (const [name, { resolve, reject }] of names) {
          const found = answer.ok ? answer.found[name] : undefined;
          if (found) {
            resolve(freeze(found));
          } else {
            answers.delete(`${ecosystem}:${name}`);
            reject(new Error(answer.ok ? `no answer for ${name}` : answer.error));
          }
        }
      });
  }
}

/** Package releases from npm, crates.io and PyPI, oldest first. */
export const registry = Object.freeze({
  npm: (name: string) => lookup("npm", name),
  cargo: (name: string) => lookup("cargo", name),
  pypi: (name: string) => lookup("pypi", name),
});

/** Versions and ranges, by npm rules, Cargo's (`{ ecosystem: "cargo" }`) or PEP 440 (`{ ecosystem: "pypi" }`). */
export const semver = makeSemver((op, ecosystem, a, b) =>
  JSON.parse(host().semver(op, ecosystem, a, b)),
);

/** Removed; kept so a file that still imports it fails with what to write instead. */
export function dependencyGraph(): never {
  throw new Error(
    'dependencyGraph() was removed: call dependencies() from "codemod/deps" inside a step, as step(() => { const deps = dependencies(); ... }); `codemod workflow upgrade` rewrites it',
  );
}

/** Starts a step, which only the workflow body can do. */
export function install(): never {
  throw new Error("install() is a step: call it in the workflow body, as await step(install())");
}

// ------------------------------------------------------------ dependencies ---

const KINDS = ["prod", "dev", "peer", "optional", "build"];
/** The declaration `declaration(name)` picks first: runtime kinds before dev. */
const PREFERRED = ["prod", "optional", "peer", "dev", "build"];
const RANGE_GROUPS = [
  "dependencies",
  "peerDependencies",
  "optionalDependencies",
  "buildDependencies",
];

type HostAnswer<T> = { ok: true; value: T } | { ok: false; error: string };

/** A host answer's value, or its error thrown as it is. */
function ask<T>(json: string): T {
  const answer = JSON.parse(json) as HostAnswer<T>;
  if (!answer.ok) throw new Error(answer.error);
  return answer.value;
}

interface KindOptions {
  kinds?: string | string[];
}

interface DependentOptions extends KindOptions {
  /** Also list third-party packages; by default only the repository's own. */
  includeThirdParty?: boolean;
}

function kindList(options?: KindOptions): string[] | undefined {
  const kinds = options?.kinds;
  if (kinds === undefined) return undefined;
  const list = typeof kinds === "string" ? [kinds] : kinds;
  if (!Array.isArray(list)) throw new TypeError("kinds must be a kind or an array of kinds");
  for (const kind of list) {
    if (!KINDS.includes(kind)) {
      throw new TypeError(`unknown dependency kind '${kind}'; use ${KINDS.join(", ")}`);
    }
  }
  return list;
}

function kindsOf(options?: KindOptions): string {
  return kindList(options)?.join(",") ?? "";
}

function name(value: unknown, what = "name"): string {
  if (typeof value !== "string" || value === "") {
    throw new TypeError(`${what} must be a package name`);
  }
  return value;
}

function rangeText(value: unknown, what = "range"): string {
  if (typeof value !== "string" || value === "") {
    throw new TypeError(`${what} must be a non-empty string, such as "^1.2.0"`);
  }
  return value;
}

/** Whether a release's own ranges accept every `{ name: version }` in `accepting`. */
function accepts(
  release: Release,
  accepting: Record<string, string>,
  options: { ecosystem: Ecosystem },
): boolean {
  for (const [dependency, version] of Object.entries(accepting)) {
    for (const group of RANGE_GROUPS) {
      const ranges = release[group] as Record<string, string> | undefined;
      const range = ranges?.[dependency];
      if (range !== undefined && semver.satisfies(version, range, options) === false) {
        return false;
      }
    }
  }
  return true;
}

interface DeclarationRecord {
  name: string;
  range: string;
  kind: string;
  protocol: string;
  resolved: string | null;
  catalog: { name: string; file: string } | null;
  local: string | null;
  readable: boolean;
}

interface PackageRecord {
  dir: string;
  manifest: string;
  name: string | null;
  version: string | null;
  declarations: DeclarationRecord[];
  member: boolean;
}

interface ProjectRecord {
  packageManager: Record<string, unknown> | null;
  packages: PackageRecord[];
  catalogs: { name: string; file: string; entries: Record<string, string> }[];
  unreadable: [string, string][];
}

interface NodeRecord {
  id: number;
  ecosystem: Ecosystem;
  name: string;
  version: string;
  dir?: string | null;
}

interface EdgeRecord {
  from: number;
  to: number | null;
  name: string;
  range?: string | null;
  kind: string;
}

interface ReleaseOptions {
  accepting?: Record<string, string>;
  newerThanMine?: boolean;
  prereleases?: boolean;
}

interface Scope extends KindOptions {
  in?: unknown;
}

interface AllOption {
  /** Every package.json under the target, not just the root and its workspaces. */
  all?: boolean;
}

/** A declaration an edit left alone, and why. */
interface SkippedEdit {
  file: string;
  package: string;
  name: string;
  kind: string;
  range: string;
  reason: string;
}

function allOf(options: unknown, what: string): boolean {
  if (options === undefined) return false;
  if (options === null || typeof options !== "object") {
    throw new TypeError(`${what}: options must be an object`);
  }
  const { all, ...rest } = options as AllOption;
  const unknown = Object.keys(rest);
  if (unknown.length) throw new TypeError(`${what}: unknown option '${unknown[0]}'`);
  if (all !== undefined && typeof all !== "boolean") {
    throw new TypeError(`${what}: all must be a boolean`);
  }
  return all === true;
}

/** One object per isolate and options: later calls in the step get it back. */
const made = new Map<string, ReturnType<typeof createDependencies>>();

/**
 * The repository's dependencies: its package.json files now, its lockfiles
 * when a graph method is first used.
 */
export function dependencies(options?: { lockfiles?: string | string[] }) {
  if (options !== undefined && (options === null || typeof options !== "object")) {
    throw new TypeError("dependencies(options): options must be an object");
  }
  const { lockfiles, ...rest } = options ?? {};
  const unknown = Object.keys(rest);
  if (unknown.length) throw new TypeError(`dependencies: unknown option '${unknown[0]}'`);
  let only: string[] = [];
  if (lockfiles !== undefined) {
    only = typeof lockfiles === "string" ? [lockfiles] : lockfiles;
    if (!Array.isArray(only) || only.some((glob) => typeof glob !== "string")) {
      throw new TypeError("dependencies: lockfiles must be a glob or an array of globs");
    }
  }
  const key = JSON.stringify(only);
  let found = made.get(key);
  if (!found) {
    found = createDependencies(only);
    made.set(key, found);
  }
  return found;
}

function createDependencies(only: string[]) {
  const host = globalThis.__dependencyHost;
  let project: ProjectRecord = { packageManager: null, packages: [], catalogs: [], unreadable: [] };
  /** Every package.json under the target. */
  let all: Package[] = [];
  /** The root and the workspaces it declares. */
  let packages: Package[] = [];
  const edits: unknown[] = [];

  const read = () => {
    project = ask<ProjectRecord>(host.project());
    all = project.packages.map((record) => new Package(record));
    packages = all.filter((pkg) => pkg.member);
  };

  const packageByDir = (dir: string) => all.find((pkg) => pkg.dir === dir);
  const packageRef = (ref: unknown): Package => {
    if (ref instanceof Package) return packageByDir(ref.dir) ?? ref;
    if (typeof ref !== "string") throw new TypeError("expected a package, its dir or its name");
    const found = deps.package(ref);
    if (!found) throw new Error(`no package '${ref}' (by dir or name) in this repository`);
    return found;
  };
  const scoped = (scope?: Scope): Package[] => {
    if (scope?.in === undefined) return packages;
    const list = Array.isArray(scope.in) ? scope.in : [scope.in];
    return list.map(packageRef);
  };
  const root = (): Package => {
    const found = packageByDir(".");
    if (!found) throw new Error("no package.json at the target's root");
    return found;
  };

  /** The edits, as an array, with what was left alone as its `skipped`. */
  const result = (done: unknown[], skipped: SkippedEdit[]) => {
    const list = [...done];
    Object.defineProperty(list, "skipped", {
      value: Object.freeze(skipped.map((skip) => Object.freeze(skip))),
    });
    // A step's result records the edits.
    Object.defineProperty(list, "toJSON", { value: () => [...done] });
    return Object.freeze(list);
  };

  const apply = (changes: Record<string, unknown>[], skipped: SkippedEdit[] = []) => {
    if (!changes.length) return result([], skipped);
    const done = ask<{ edits: Record<string, unknown>[]; skipped: SkippedEdit[] }>(
      host.edit(JSON.stringify(changes)),
    );
    const frozen = done.edits.map((edit) => Object.freeze(edit));
    edits.push(...frozen);
    read();
    return result(frozen, [...skipped, ...done.skipped]);
  };

  /**
   * The declarations an edit of `name` changes: peers only when `kinds`
   * names "peer"; the peers it leaves are reported as skipped.
   */
  const editable = (dependency: string, options?: Scope) => {
    const peers = kindList(options)?.includes("peer") ?? false;
    const kept: Declaration[] = [];
    const skipped: SkippedEdit[] = [];
    for (const d of deps.declarations(dependency, options)) {
      if (d.kind === "peer" && !peers) {
        skipped.push({
          file: d.package.manifest,
          package: d.package.dir,
          name: d.name,
          kind: d.kind,
          range: d.range,
          reason: "peer",
        });
      } else {
        kept.push(d);
      }
    }
    return { kept, skipped };
  };

  class Declaration {
    readonly package: Package;
    readonly name: string;
    readonly range: string;
    readonly kind: string;
    readonly protocol: string;
    readonly resolved: string | null;
    readonly catalog: { readonly name: string; readonly file: string } | null;
    readonly readable: boolean;
    readonly #local: string | null;

    constructor(owner: Package, record: DeclarationRecord) {
      this.package = owner;
      this.name = record.name;
      this.range = record.range;
      this.kind = record.kind;
      this.protocol = record.protocol;
      this.resolved = record.resolved;
      this.catalog = record.catalog ? Object.freeze({ ...record.catalog }) : null;
      this.readable = record.readable;
      this.#local = record.local;
      Object.freeze(this);
    }

    /** The local package a `workspace:` ref points to. */
    get local(): Package | null {
      return this.#local === null ? null : (packageByDir(this.#local) ?? null);
    }

    /** Whether `resolved` accepts `version`: true, false, or null when it cannot tell. */
    accepts(version: string): boolean | null {
      if (this.resolved === null || !this.readable) return null;
      return semver.satisfies(version, this.resolved);
    }

    /** The lowest version `resolved` accepts; null when unreadable. */
    minVersion(): string | null {
      if (this.resolved === null || !this.readable) return null;
      return semver.minVersion(this.resolved);
    }

    /** What the lockfile installs for this declaration; null when it does not resolve it. */
    installed() {
      const node = graph().workspaceNode(this.package.dir);
      if (!node) return null;
      const edge = node
        .dependencies({ kinds: this.kind })
        .find((candidate) => candidate.name === this.name);
      return edge?.to ?? null;
    }

    toJSON() {
      return {
        package: this.package.dir,
        name: this.name,
        range: this.range,
        kind: this.kind,
        resolved: this.resolved,
      };
    }
  }

  class Package {
    readonly ecosystem = "npm";
    readonly name: string | null;
    readonly version: string | null;
    readonly dir: string;
    readonly manifest: string;
    /** The root, or a workspace the root declares. */
    readonly member: boolean;
    readonly #declarations: Declaration[];

    constructor(record: PackageRecord) {
      this.name = record.name;
      this.version = record.version;
      this.dir = record.dir;
      this.manifest = record.manifest;
      this.member = record.member;
      this.#declarations = record.declarations.map((d) => new Declaration(this, d));
      Object.freeze(this);
    }

    /** Every declaration, in manifest order. */
    declarations(options?: KindOptions): Declaration[] {
      const kinds = kindList(options);
      return this.#declarations.filter((d) => !kinds || kinds.includes(d.kind));
    }

    /** The declaration of `name`, runtime kinds before dev. */
    declaration(dependency: string, options?: KindOptions): Declaration | undefined {
      const found = this.declarations(options).filter((d) => d.name === dependency);
      found.sort((a, b) => PREFERRED.indexOf(a.kind) - PREFERRED.indexOf(b.kind));
      return found[0];
    }

    uses(dependency: string, options?: KindOptions): boolean {
      return this.declaration(name(dependency), options) !== undefined;
    }

    add(dependency: string, range: string, options?: { kind?: string }) {
      return deps.add(dependency, range, { ...options, to: this });
    }

    bump(dependency: string, range: unknown, options?: KindOptions) {
      return deps.bump(dependency, range, { ...options, in: this });
    }

    remove(dependency: string, options?: KindOptions) {
      return deps.remove(dependency, { ...options, in: this });
    }

    rename(from: string, to: string, options?: KindOptions & { range?: string }) {
      return deps.rename(from, to, { ...options, in: this });
    }

    toString(): string {
      if (this.name === null) return this.dir;
      return this.version === null ? this.name : `${this.name}@${this.version}`;
    }

    toJSON(): string {
      return this.toString();
    }
  }

  let loaded: ReturnType<typeof loadGraph> | undefined;
  const graph = () => (loaded ??= loadGraph(only));

  const deps = {
    get packageManager() {
      return project.packageManager;
    },

    /**
     * The repository's packages: the root, then the workspaces it declares,
     * by dir. `{ all: true }`: every package.json under the target.
     */
    packages: (options?: AllOption): Package[] =>
      allOf(options, "packages") ? [...all] : [...packages],

    /** A package by dir, or by name (the repository's packages first). */
    package: (ref: string): Package | undefined => {
      if (typeof ref !== "string") throw new TypeError("package(ref): give a dir or a name");
      const dir = ref.replace(/^\.\//, "").replace(/\/$/, "") || ".";
      return (
        packageByDir(dir) ??
        packages.find((pkg) => pkg.name === ref) ??
        all.find((pkg) => pkg.name === ref)
      );
    },

    /**
     * The package that owns a file: the nearest of the repository's
     * packages at or above it, or of every package.json with `{ all: true }`.
     */
    packageOf: (file: string, options?: AllOption): Package | undefined => {
      if (typeof file !== "string") throw new TypeError("packageOf(file): give a path");
      const path = file.replace(/\\/g, "/").replace(/^\.\//, "");
      let best: Package | undefined;
      for (const pkg of allOf(options, "packageOf") ? all : packages) {
        const inside = pkg.dir === "." || path === pkg.dir || path.startsWith(`${pkg.dir}/`);
        if (!inside) continue;
        if (!best || best.dir === "." || pkg.dir.length > best.dir.length) best = pkg;
      }
      return best;
    },

    /** Whether any of the repository's packages declares `name`. */
    uses: (dependency: string, options?: KindOptions): boolean =>
      packages.some((pkg) => pkg.uses(dependency, options)),

    /** Every declaration of `name` across the packages (or those `in`). */
    declarations: (dependency: string, options?: Scope): Declaration[] => {
      name(dependency);
      return scoped(options).flatMap((pkg) =>
        pkg.declarations(options).filter((d) => d.name === dependency),
      );
    },

    /** Catalogs: name, file and entries. */
    catalogs: () => project.catalogs.map((catalog) => Object.freeze({ ...catalog })),

    /** Set the range of `name` where it is declared (peers only when `kinds` names them). */
    bump: (dependency: string, range: unknown, options?: Scope) => {
      if (typeof range !== "function") rangeText(range);
      const { kept, skipped } = editable(dependency, options);
      const changes = kept.flatMap((d) => {
        const next =
          typeof range === "function" ? (range as (d: Declaration) => unknown)(d) : range;
        if (next === undefined) return [];
        return [
          { op: "set", dir: d.package.dir, kind: d.kind, name: d.name, range: rangeText(next) },
        ];
      });
      return apply(changes, skipped);
    },

    /** Declare `name` in a package (default: the root) and section (default: prod). */
    add: (dependency: string, range: string, options?: { to?: unknown; kind?: string }) => {
      const kind = options?.kind ?? "prod";
      kindList({ kinds: kind });
      const target = options?.to === undefined ? root() : packageRef(options.to);
      return apply([
        { op: "add", dir: target.dir, kind, name: name(dependency), range: rangeText(range) },
      ]);
    },

    /** Remove `name` wherever it is declared (peers only when `kinds` names them). */
    remove: (dependency: string, options?: Scope) => {
      const { kept, skipped } = editable(dependency, options);
      return apply(
        kept.map((d) => ({ op: "remove", dir: d.package.dir, kind: d.kind, name: d.name })),
        skipped,
      );
    },

    /** Rename `from` to `to` where declared, keeping the section and range unless `range` is given. */
    rename: (from: string, to: string, options?: Scope & { range?: string }) => {
      name(to, "to");
      const range = options?.range === undefined ? undefined : rangeText(options.range);
      const { kept, skipped } = editable(from, options);
      return apply(
        kept.map((d) => ({ op: "rename", dir: d.package.dir, kind: d.kind, from, to, range })),
        skipped,
      );
    },

    /** Every edit made through this object, in order. */
    get edits() {
      return Object.freeze([...edits]);
    },

    /** Whether the lockfile is behind the manifests. Loads the graph. */
    needsInstall: (): boolean => {
      const g = graph();
      for (const pkg of packages) {
        const declared = pkg.declarations().filter((d) => d.kind !== "peer");
        const node = g.workspaceNode(pkg.dir);
        if (!node) {
          if (declared.length) return true;
          continue;
        }
        const edges = node.dependencies().filter((e) => e.kind !== "peer");
        for (const d of declared) {
          const edge = edges.find((e) => e.name === d.name && e.kind === d.kind);
          if (!edge) return true;
          if (edge.range !== null && edge.range !== d.range) return true;
          if (edge.to === null && d.kind !== "optional") return true;
        }
        for (const edge of edges) {
          if (!declared.some((d) => d.name === edge.name && d.kind === edge.kind)) return true;
        }
      }
      return false;
    },

    get lockfiles(): readonly string[] {
      return graph().lockfiles;
    },
    get skipped() {
      return graph().skipped;
    },
    /** Installed versions of `name`, lowest first. */
    installed: (dependency: string): string[] => graph().installed(name(dependency)),
    workspaces: () => graph().workspaces(),
    nodes: (dependency?: string) => graph().nodes(dependency),
    node: (dependency: string, version?: string) => graph().node(dependency, version),
    dependents: (dependency: string, options?: DependentOptions) =>
      graph().dependents(dependency, options),
    chains: (options: { from?: unknown; to?: unknown; kinds?: string | string[] }) =>
      graph().chains(options, (ref) => (ref instanceof Package ? ref.dir : undefined)),
  };

  read();
  return Object.freeze(deps);
}

/** The lockfile graph over host node ids; see deps_host.rs. */
function loadGraph(only: string[]) {
  const host = globalThis.__dependencyHost;
  const loaded = ask<{
    id: number;
    lockfiles: string[];
    skipped: { lockfile: string; error: string }[];
  }>(host.graph(JSON.stringify(only)));
  const gid = loaded.id;
  const ids = new WeakMap<object, number>();
  const nodes = new Map<number, PackageNode>();

  const nodeOf = (record: NodeRecord): PackageNode => {
    let node = nodes.get(record.id);
    if (!node) {
      node = new PackageNode(record);
      nodes.set(record.id, node);
    }
    return node;
  };
  const byId = (id: number): PackageNode =>
    nodes.get(id) ?? nodeOf(JSON.parse(host.node(gid, id)) as NodeRecord);
  const idOf = (node: PackageNode): number => {
    const id = ids.get(node);
    if (id === undefined) throw new TypeError("expected a node from this graph");
    return id;
  };
  const edges = (direction: string, key: string | number, options?: KindOptions) =>
    ask<EdgeRecord[]>(host.edges(gid, direction, String(key), kindsOf(options))).map(
      (edge) => new DependencyEdge(edge),
    );
  /** Edges onto a package, from the repository's own packages unless `includeThirdParty`. */
  const dependentEdges = (direction: string, key: string | number, options?: DependentOptions) => {
    const found = edges(direction, key, options);
    return options?.includeThirdParty ? found : found.filter((edge) => edge.from.local);
  };
  const nodesOf = (json: string) => (JSON.parse(json) as NodeRecord[]).map(nodeOf);

  class PackageNode {
    readonly ecosystem: Ecosystem;
    readonly name: string;
    readonly version: string;
    /** Directory of one of the repository's own packages; null for installed packages. */
    readonly dir: string | null;
    /** Whether this is one of the repository's own packages. */
    readonly local: boolean;

    constructor(record: NodeRecord) {
      ids.set(this, record.id);
      this.ecosystem = record.ecosystem;
      this.name = record.name;
      this.version = record.version;
      this.dir = record.dir ?? null;
      this.local = this.dir !== null;
      Object.freeze(this);
    }

    /** Edges this node declares. */
    dependencies(options?: KindOptions): DependencyEdge[] {
      return edges("out", idOf(this), options);
    }

    /** Edges installed onto this exact node, from the repository's own packages unless `includeThirdParty`. */
    installedBy(options?: DependentOptions): DependencyEdge[] {
      return dependentEdges("in", idOf(this), options);
    }

    /** Releases of this package from the registry, oldest first, each with `breaking` relative to this node's version. */
    async releases({ accepting, newerThanMine = false, prereleases = false }: ReleaseOptions = {}) {
      const options = { ecosystem: this.ecosystem };
      if (accepting && this.ecosystem === "pypi") {
        throw new TypeError(
          "accepting cannot filter PyPI releases: PyPI lists no dependencies per release",
        );
      }
      const found = await registry[this.ecosystem](this.name);
      return found.versions
        .filter(
          (release) =>
            !release.yanked &&
            (prereleases || !release.prerelease) &&
            (!newerThanMine || semver.gt(release.version, this.version, options)) &&
            (!accepting || accepts(release, accepting, options)),
        )
        .map((release) =>
          Object.freeze({
            ...release,
            breaking: semver.isBreaking(this.version, release.version, options),
          }),
        );
    }

    toString(): string {
      return `${this.name}@${this.version}`;
    }

    toJSON(): string {
      return this.toString();
    }
  }

  class DependencyEdge {
    readonly from: PackageNode;
    /** The installed node; null when the lockfile does not resolve it, as for most peer edges. */
    readonly to: PackageNode | null;
    readonly name: string;
    /** Declared range; null when the lockfile keeps none (third-party crates in Cargo.lock). */
    readonly range: string | null;
    readonly kind: string;

    constructor(record: EdgeRecord) {
      this.from = byId(record.from);
      this.to = record.to === null ? null : byId(record.to);
      this.name = record.name;
      this.range = record.range ?? null;
      this.kind = record.kind;
      Object.freeze(this);
    }

    /** Whether the declared range accepts `version`: true, false, or null when unknown. */
    accepts(version: string): boolean | null {
      if (this.range === null) return null;
      return semver.satisfies(version, this.range, { ecosystem: this.from.ecosystem });
    }

    toJSON() {
      return {
        from: this.from.toJSON(),
        to: this.to ? this.to.toJSON() : this.name,
        kind: this.kind,
        range: this.range,
      };
    }
  }

  const graph = {
    lockfiles: Object.freeze(loaded.lockfiles),
    skipped: Object.freeze(loaded.skipped.map((s) => Object.freeze(s))),

    workspaces: (): PackageNode[] => nodesOf(host.nodes(gid, "workspaces", "")),

    nodes: (dependency?: string): PackageNode[] =>
      nodesOf(
        dependency === undefined
          ? host.nodes(gid, "all", "")
          : host.nodes(gid, "named", dependency),
      ),

    /** One node by name, and by version when several are installed; throws when several are and no version is given. */
    node: (dependency: string, version?: string): PackageNode | undefined => {
      const named = graph.nodes(dependency);
      if (version !== undefined) return named.find((node) => node.version === version);
      const versions = [...new Set(named.map((node) => node.version))];
      if (versions.length > 1) {
        throw new Error(
          `node("${dependency}"): ${versions.length} versions are installed (${versions.join(", ")}); use nodes("${dependency}") or pass a version`,
        );
      }
      return named[0];
    },

    /** Installed (not local) versions of a package, lowest first. */
    installed: (dependency: string): string[] => {
      const found = graph.nodes(dependency).filter((node) => !node.local);
      const versions = [...new Set(found.map((node) => node.version))];
      const ecosystem = found[0]?.ecosystem ?? "npm";
      return versions.sort((a, b) => semver.compare(a, b, { ecosystem }));
    },

    /** The lockfile's node for one of the repository's npm packages, by dir. */
    workspaceNode: (dir: string): PackageNode | undefined =>
      graph.workspaces().find((node) => node.dir === dir && node.ecosystem === "npm"),

    dependents: (dependency: string, options?: DependentOptions): DependencyEdge[] =>
      dependentEdges("named", dependency, options),

    /**
     * The shortest chain from each `from` (default: every workspace) to any `to`.
     * A name means every installed version of it; a package, its workspace node.
     */
    chains: (
      { from, to, kinds }: { from?: unknown; to?: unknown; kinds?: string | string[] } = {},
      packageDir: (ref: unknown) => string | undefined = () => undefined,
    ): readonly PackageNode[][] => {
      if (to === undefined) {
        throw new TypeError("chains({ to }): name the packages or nodes to reach");
      }
      const resolve = (refs: unknown): PackageNode[] =>
        (Array.isArray(refs) ? refs : [refs]).flatMap((ref): PackageNode[] => {
          if (typeof ref === "string") return graph.nodes(ref);
          if (ref instanceof PackageNode) return [ref];
          const dir = packageDir(ref);
          if (dir !== undefined) {
            const node = graph.workspaceNode(dir);
            return node ? [node] : [];
          }
          throw new TypeError("chains: from and to take package names, packages or nodes");
        });
      const starts = from === undefined ? graph.workspaces() : resolve(from);
      const targets = resolve(to);
      if (!starts.length || !targets.length) return [];
      const found = ask<number[][]>(
        host.chains(
          gid,
          JSON.stringify(starts.map(idOf)),
          JSON.stringify(targets.map(idOf)),
          kindsOf({ kinds }),
        ),
      );
      return found.map((chain) => Object.freeze(chain.map(byId)) as PackageNode[]);
    },
  };
  return Object.freeze(graph);
}

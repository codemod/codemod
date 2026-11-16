// `codemod-internal:dependencies`: `registry`, `semver` and the dependency
// graph inside isolates (function steps, forEachFile and dependencyGraph
// callbacks). The build imports `registry` and `semver` from here when a
// step or callback uses them. The host installs
// `globalThis.__dependencyHost` in every isolate and
// `globalThis.__graphHost` for dependencyGraph callbacks (`globals.d.ts`).

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

/** Package releases from npm and crates.io, oldest first. */
export const registry = Object.freeze({
  npm: (name: string) => lookup("npm", name),
  cargo: (name: string) => lookup("cargo", name),
});

/** Versions and ranges, by npm rules or (with `{ ecosystem: "cargo" }`) Cargo's. */
export const semver = makeSemver((op, ecosystem, a, b) =>
  JSON.parse(host().semver(op, ecosystem, a, b)),
);

// ------------------------------------------------------------------ graph ---

const KINDS = ["prod", "dev", "peer", "optional", "build"];
const RANGE_GROUPS = [
  "dependencies",
  "peerDependencies",
  "optionalDependencies",
  "buildDependencies",
];

interface KindOptions {
  kinds?: string | string[];
}

function kindsOf(options?: KindOptions): string {
  const kinds = options?.kinds;
  if (kinds === undefined) return "";
  const list = typeof kinds === "string" ? [kinds] : kinds;
  if (!Array.isArray(list)) throw new TypeError("kinds must be a kind or an array of kinds");
  for (const kind of list) {
    if (!KINDS.includes(kind)) {
      throw new TypeError(`unknown dependency kind '${kind}'; use ${KINDS.join(", ")}`);
    }
  }
  return list.join(",");
}

/** Whether a release's own ranges accept every `{ name: version }` in `accepting`. */
function accepts(
  release: Release,
  accepting: Record<string, string>,
  options: { ecosystem: Ecosystem },
): boolean {
  for (const [name, version] of Object.entries(accepting)) {
    for (const group of RANGE_GROUPS) {
      const ranges = release[group] as Record<string, string> | undefined;
      const range = ranges?.[name];
      if (range !== undefined && semver.satisfies(version, range, options) === false) {
        return false;
      }
    }
  }
  return true;
}

interface NodeRecord {
  id: number;
  ecosystem: Ecosystem;
  name: string;
  version: string;
  workspace?: string | null;
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

/** The live dependency graph of a `dependencyGraph` step; see tasks/dependency_graph.rs. */
export function __graph() {
  const host = globalThis.__graphHost;
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
    nodes.get(id) ?? nodeOf(JSON.parse(host.node(id)) as NodeRecord);
  const idOf = (node: PackageNode): number => {
    const id = ids.get(node);
    if (id === undefined) throw new TypeError("expected a node from this graph");
    return id;
  };
  const many = (value: PackageNode | PackageNode[]): PackageNode[] =>
    Array.isArray(value) ? value : [value];
  const edges = (direction: string, key: string | number, options?: KindOptions) =>
    (JSON.parse(host.edges(direction, String(key), kindsOf(options))) as EdgeRecord[]).map(
      (edge) => new DependencyEdge(edge),
    );
  const nodesOf = (json: string) => (JSON.parse(json) as NodeRecord[]).map(nodeOf);

  class PackageNode {
    readonly ecosystem: Ecosystem;
    readonly name: string;
    readonly version: string;
    /** Directory of one of the repository's own packages; null for third-party packages. */
    readonly workspace: string | null;

    constructor(record: NodeRecord) {
      ids.set(this, record.id);
      this.ecosystem = record.ecosystem;
      this.name = record.name;
      this.version = record.version;
      this.workspace = record.workspace ?? null;
      Object.freeze(this);
    }

    get isWorkspace(): boolean {
      return this.workspace !== null;
    }

    /** Edges this node declares. */
    dependencies(options?: KindOptions): DependencyEdge[] {
      return edges("out", idOf(this), options);
    }

    /** Edges installed onto this exact node. */
    dependents(options?: KindOptions): DependencyEdge[] {
      return edges("in", idOf(this), options);
    }

    /** Each workspace that reaches this node, with its shortest chain (workspace first). */
    workspacesReaching(options?: KindOptions) {
      return graph
        .chains({ from: graph.workspaces(), to: this, kinds: options?.kinds })
        .map((chain) => Object.freeze({ workspace: chain[0], chain }));
    }

    /**
     * Releases of this package from the registry, oldest first, each with
     * `breaking` relative to this node's version.
     *   accepting:    { react: "19.0.0" } keeps releases whose ranges on react accept 19.0.0
     *   newerThanMine: keep releases newer than this node's version
     *   prereleases:  keep prereleases (dropped by default)
     */
    async releases({ accepting, newerThanMine = false, prereleases = false }: ReleaseOptions = {}) {
      const options = { ecosystem: this.ecosystem };
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
    /** The installed node; null for peer edges and ranges the lockfile does not resolve. */
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

  interface ChainOptions extends KindOptions {
    from?: PackageNode | PackageNode[];
    to?: PackageNode | PackageNode[];
  }

  const graph = Object.freeze({
    lockfiles: Object.freeze(JSON.parse(host.lockfiles) as string[]),

    /** The repository's own packages. */
    workspaces: (): PackageNode[] => nodesOf(host.nodes("workspaces", "")),

    /** Every node, or every installed version of `name`. */
    nodes: (name?: string): PackageNode[] =>
      nodesOf(name === undefined ? host.nodes("all", "") : host.nodes("named", name)),

    /** One node by name, and version when several are installed. */
    node: (name: string, version?: string): PackageNode | undefined =>
      graph.nodes(name).find((node) => version === undefined || node.version === version),

    /** Edges that declare a dependency on `name`, resolved or not (peer edges are never resolved). */
    dependents: (name: string, options?: KindOptions): DependencyEdge[] =>
      edges("named", name, options),

    /** Shortest chains from each `from` node (default: every workspace) to any `to` node. */
    chains: ({ from, to, kinds }: ChainOptions = {}): readonly PackageNode[][] => {
      if (to === undefined) throw new TypeError("chains({ to }): name the node or nodes to reach");
      const starts = from === undefined ? graph.workspaces() : many(from);
      const found = JSON.parse(
        host.chains(
          JSON.stringify(starts.map(idOf)),
          JSON.stringify(many(to).map(idOf)),
          kindsOf({ kinds }),
        ),
      ) as number[][];
      return found.map((chain) => Object.freeze(chain.map(byId)) as PackageNode[]);
    },
  });
  return graph;
}

// `codemod:workflow/dependencies`: `registry`, `semver` and the dependency
// graph inside isolates (function steps, forEachFile and dependencyGraph
// callbacks). The build imports `registry` and `semver` from here when a
// step or callback uses them. The host installs
// `globalThis.__dependencyHost` in every isolate and
// `globalThis.__graphHost` for dependencyGraph callbacks.

import { makeSemver } from "codemod:workflow/semver";

const host = () => globalThis.__dependencyHost;

function freeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const key of Object.keys(value)) freeze(value[key]);
  }
  return value;
}

// Lookups started in the same turn go to the host as one batch per
// ecosystem, so `Promise.all` over many packages fetches them together.
// Each package is asked once per isolate; the host caches across the run.
const answers = new Map();
let queued = null;

function lookup(ecosystem, name) {
  if (typeof name !== "string" || name === "") {
    return Promise.reject(new TypeError(`registry.${ecosystem}(name) expects a package name`));
  }
  const key = `${ecosystem}:${name}`;
  let answer = answers.get(key);
  if (!answer) {
    answer = new Promise((resolve, reject) => {
      if (!queued) {
        queued = new Map();
        Promise.resolve().then(flush);
      }
      const names = queued.get(ecosystem) ?? new Map();
      names.set(name, { resolve, reject });
      queued.set(ecosystem, names);
    });
    answers.set(key, answer);
  }
  return answer;
}

function flush() {
  const batch = queued;
  queued = null;
  for (const [ecosystem, names] of batch) {
    host()
      .releases(ecosystem, JSON.stringify([...names.keys()]))
      .then((json) => JSON.parse(json))
      .catch((error) => ({ ok: false, error: String(error && error.message ? error.message : error) }))
      .then((answer) => {
        for (const [name, { resolve, reject }] of names) {
          if (answer.ok) {
            resolve(freeze(answer.found[name]));
          } else {
            answers.delete(`${ecosystem}:${name}`);
            reject(new Error(answer.error));
          }
        }
      });
  }
}

/** Package releases from npm and crates.io, oldest first. */
export const registry = Object.freeze({
  npm: (name) => lookup("npm", name),
  cargo: (name) => lookup("cargo", name),
});

/** Versions and ranges, by npm rules or (with `{ ecosystem: "cargo" }`) Cargo's. */
export const semver = makeSemver((op, ecosystem, a, b) =>
  JSON.parse(host().semver(op, ecosystem, a, b)),
);

// ------------------------------------------------------------------ graph ---

const __KINDS = ["prod", "dev", "peer", "optional", "build"];
const __RANGE_GROUPS = ["dependencies", "peerDependencies", "optionalDependencies", "buildDependencies"];

function __kinds(options) {
  const kinds = options && options.kinds;
  if (kinds === undefined) return "";
  const list = typeof kinds === "string" ? [kinds] : kinds;
  if (!Array.isArray(list)) throw new TypeError("kinds must be a kind or an array of kinds");
  for (const kind of list) {
    if (!__KINDS.includes(kind)) {
      throw new TypeError(`unknown dependency kind '${kind}'; use ${__KINDS.join(", ")}`);
    }
  }
  return list.join(",");
}

/** Whether a release's own ranges accept every `{ name: version }` in `accepting`. */
function __accepts(release, accepting, options) {
  for (const [name, version] of Object.entries(accepting)) {
    for (const group of __RANGE_GROUPS) {
      const range = release[group] && release[group][name];
      if (range !== undefined && semver.satisfies(version, range, options) === false) {
        return false;
      }
    }
  }
  return true;
}

/** The live dependency graph of a `dependencyGraph` step; see tasks/dependency_graph.rs. */
export function __graph() {
  const host = globalThis.__graphHost;
  const ids = new WeakMap();
  const nodes = new Map();

  const nodeOf = (record) => {
    let node = nodes.get(record.id);
    if (!node) {
      node = new PackageNode(record);
      nodes.set(record.id, node);
    }
    return node;
  };
  const byId = (id) => nodes.get(id) ?? nodeOf(JSON.parse(host.node(id)));
  const idOf = (node) => {
    const id = ids.get(node);
    if (id === undefined) throw new TypeError("expected a node from this graph");
    return id;
  };
  const many = (value) => (Array.isArray(value) ? value : [value]);
  const edges = (direction, key, options) =>
    JSON.parse(host.edges(direction, String(key), __kinds(options))).map(
      (edge) => new DependencyEdge(edge),
    );

  class PackageNode {
    constructor(record) {
      ids.set(this, record.id);
      this.ecosystem = record.ecosystem;
      this.name = record.name;
      this.version = record.version;
      /** Directory of one of the repository's own packages; null for third-party packages. */
      this.workspace = record.workspace ?? null;
      Object.freeze(this);
    }

    get isWorkspace() {
      return this.workspace !== null;
    }

    /** Edges this node declares. */
    dependencies(options) {
      return edges("out", idOf(this), options);
    }

    /** Edges installed onto this exact node. */
    dependents(options) {
      return edges("in", idOf(this), options);
    }

    /** Each workspace that reaches this node, with its shortest chain (workspace first). */
    workspacesReaching(options) {
      return graph
        .chains({ from: graph.workspaces(), to: this, kinds: options && options.kinds })
        .map((chain) => Object.freeze({ workspace: chain[0], chain }));
    }

    /**
     * Releases of this package from the registry, oldest first, each with
     * `breaking` relative to this node's version.
     *   accepting:    { react: "19.0.0" } keeps releases whose ranges on react accept 19.0.0
     *   newerThanMine: keep releases newer than this node's version
     *   prereleases:  keep prereleases (dropped by default)
     */
    async releases({ accepting, newerThanMine = false, prereleases = false } = {}) {
      const options = { ecosystem: this.ecosystem };
      const found = await registry[this.ecosystem](this.name);
      return found.versions
        .filter(
          (release) =>
            !release.yanked &&
            (prereleases || !release.prerelease) &&
            (!newerThanMine || semver.gt(release.version, this.version, options)) &&
            (!accepting || __accepts(release, accepting, options)),
        )
        .map((release) =>
          Object.freeze({
            ...release,
            breaking: semver.isBreaking(this.version, release.version, options),
          }),
        );
    }

    toString() {
      return `${this.name}@${this.version}`;
    }

    toJSON() {
      return this.toString();
    }
  }

  class DependencyEdge {
    constructor(record) {
      this.from = byId(record.from);
      /** The installed node; null for peer edges and ranges the lockfile does not resolve. */
      this.to = record.to === null ? null : byId(record.to);
      this.name = record.name;
      /** Declared range; null when the lockfile keeps none (third-party crates in Cargo.lock). */
      this.range = record.range ?? null;
      this.kind = record.kind;
      Object.freeze(this);
    }

    /** Whether the declared range accepts `version`: true, false, or null when unknown. */
    accepts(version) {
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

  const graph = Object.freeze({
    lockfiles: Object.freeze(JSON.parse(host.lockfiles)),

    /** The repository's own packages. */
    workspaces: () => JSON.parse(host.nodes("workspaces", "")).map(nodeOf),

    /** Every node, or every installed version of `name`. */
    nodes: (name) =>
      JSON.parse(name === undefined ? host.nodes("all", "") : host.nodes("named", name)).map(
        nodeOf,
      ),

    /** One node by name, and version when several are installed. */
    node: (name, version) =>
      graph.nodes(name).find((node) => version === undefined || node.version === version),

    /** Edges that declare a dependency on `name`, resolved or not (peer edges are never resolved). */
    dependents: (name, options) => edges("named", name, options),

    /** Shortest chains from each `from` node (default: every workspace) to any `to` node. */
    chains: ({ from, to, kinds } = {}) => {
      if (to === undefined) throw new TypeError("chains({ to }): name the node or nodes to reach");
      const starts = from === undefined ? graph.workspaces() : many(from);
      const found = JSON.parse(
        host.chains(
          JSON.stringify(starts.map(idOf)),
          JSON.stringify(many(to).map(idOf)),
          __kinds({ kinds }),
        ),
      );
      return found.map((chain) => Object.freeze(chain.map(byId)));
    },
  });
  return graph;
}

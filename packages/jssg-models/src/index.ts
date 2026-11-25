/**
 * The model toolkit (experimental).
 *
 * A model looks at one file and states facts the analysis cannot read off
 * the syntax: "the framework calls this function", "this call sends an HTTP
 * request to that route". The semantic provider merges the facts into
 * `callers()`, `callees()`, `flowsTo()` and `flowsFrom()`, and marks every
 * edge it adds with the model's name.
 *
 * Nothing in the engine knows a framework. A model uses the same API a
 * codemod uses (`find`, `definition()`, `flowsFrom()`, …) plus the six
 * functions on its context.
 */

export { http } from "./http.ts";
export { stringParts, constantString } from "./values.ts";
export type { StringPart } from "./values.ts";
export { describeFacts } from "./testing.ts";

/** An ast-grep node, as a codemod sees it. */
export type SgNode = any;
/** An ast-grep root, as a codemod sees it. */
export type SgRoot = any;

/** One part of a channel key. */
export type KeyPart =
  /** Fixed text, compared exactly. */
  | string
  /** One part whose value varies; names the port that carries it ("" for none). */
  | { hole: string }
  /** One or more parts to the end. */
  | { rest: string }
  /** This part and everything after it could not be worked out. */
  | { unknown: true };

/** What producers and consumers are joined on. Build one with a helper (`http.key`). */
export interface ChannelKey {
  /** `http`, `kafka`, `graphql`, `sql`, `env`, … */
  protocol: string;
  parts: KeyPart[];
  /** The key as its protocol writes it, for people. Not compared. */
  text?: string;
}

/** Named values that cross a channel. A name may have several nodes. */
export type Ports = Record<string, SgNode | SgNode[]>;

export interface ChannelEnd {
  /** Values this end sends: a producer's path parameters and body, a consumer's response. */
  sends?: Ports;
  /** Where this end receives the other end's values. */
  receives?: Ports;
}

export interface ModelFile {
  /** Absolute path. */
  path: string;
  /** Path from the workspace root, with `/`. */
  relative: string;
}

export interface ModelContext {
  root: SgRoot;
  file: ModelFile;
  /** Code outside the workspace calls this function. */
  entry(fn: SgNode, meta: { kind: string; label?: string }): void;
  /** `from` (a call, or a function) calls the function `to`. */
  invokes(from: SgNode, to: SgNode): void;
  /** The value of `from` becomes the value of `to`. */
  flows(from: SgNode, to: SgNode): void;
  /** This call sends on a channel. */
  produce(key: ChannelKey, call: SgNode, end?: ChannelEnd): void;
  /** This function receives from a channel. */
  consume(key: ChannelKey, handler: SgNode, end?: ChannelEnd): void;
  /** Labels on a node: `source:user-input`, `sink:sql`, `pii`. */
  tag(node: SgNode, ...labels: string[]): void;
}

export interface Model {
  /** Shows up on every edge the model adds: `models: ["next-app-routes"]`. */
  name: string;
  version?: string;
  /** Globs from the workspace root. Only these files are parsed for the model. */
  files: string | string[];
  /** The file's text must contain one of these (checked before the model runs). */
  contains?: string | string[];
  model(context: ModelContext): void;
}

export function defineModel(model: Model): Model {
  if (!model.name) throw new TypeError("defineModel: a model needs a name");
  if (!model.files || model.files.length === 0) {
    throw new TypeError(`defineModel(${model.name}): \`files\` is required; a model that runs on every file is slow`);
  }
  return model;
}

type NodeRef = { file: string; start: number; end: number };

function ref(node: SgNode, what: string): NodeRef {
  if (!node || typeof node.range !== "function") {
    throw new TypeError(`${what}: expected a node, got ${node === null ? "null" : typeof node}`);
  }
  const range = node.range();
  return { file: node.getRoot().filename(), start: range.start.index, end: range.end.index };
}

function ports(value: Ports | undefined, what: string) {
  const out: { name: string; node: NodeRef }[] = [];
  for (const [name, nodes] of Object.entries(value ?? {})) {
    for (const node of Array.isArray(nodes) ? nodes : [nodes]) {
      out.push({ name, node: ref(node, `${what} port \`${name}\``) });
    }
  }
  return out;
}

/** Run one model on one root and return the facts it states. */
export function runModel(model: Model, root: SgRoot, file: ModelFile): unknown[] {
  const facts: unknown[] = [];
  const channel = (side: "producer" | "consumer") => (key: ChannelKey, node: SgNode, end: ChannelEnd = {}) => {
    facts.push({
      kind: "channel",
      side,
      key,
      node: ref(node, side === "producer" ? "produce(key, call)" : "consume(key, handler)"),
      sends: ports(end.sends, "sends"),
      receives: ports(end.receives, "receives"),
    });
  };
  model.model({
    root,
    file,
    entry: (fn, meta) =>
      facts.push({ kind: "entry", node: ref(fn, "entry(fn)"), entryKind: meta.kind, label: meta.label ?? null }),
    invokes: (from, to) =>
      facts.push({ kind: "invokes", from: ref(from, "invokes(from)"), to: ref(to, "invokes(, to)") }),
    flows: (from, to) => facts.push({ kind: "flows", from: ref(from, "flows(from)"), to: ref(to, "flows(, to)") }),
    produce: channel("producer"),
    consume: channel("consumer"),
    tag: (node, ...labels) => facts.push({ kind: "tag", node: ref(node, "tag(node)"), labels }),
  });
  return facts;
}

/** `**`, `*`, `?` and `{a,b}`; the pattern must match the whole path. */
export function globToRegExp(glob: string): RegExp {
  let source = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === "*" && glob[i + 1] === "*") {
      // `**/` also matches nothing.
      if (glob[i + 2] === "/") {
        source += "(?:.*/)?";
        i += 2;
      } else {
        source += ".*";
        i += 1;
      }
    } else if (c === "*") source += "[^/]*";
    else if (c === "?") source += "[^/]";
    else if (c === "{") source += "(?:";
    else if (c === "}") source += ")";
    else if (c === ",") source += "|";
    else source += c.replace(/[.+^$()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${source}$`);
}

function commonDirectory(paths: string[]): string {
  if (paths.length === 0) return "";
  let prefix = paths[0].slice(0, paths[0].lastIndexOf("/") + 1);
  for (const path of paths) {
    while (!path.startsWith(prefix)) {
      prefix = prefix.slice(0, prefix.lastIndexOf("/", prefix.length - 2) + 1);
    }
  }
  return prefix;
}

export interface ApplyResult {
  /** Per model: the files it ran on and the facts it stated. */
  models: { name: string; files: number; facts: number }[];
}

/**
 * Run models over the workspace and hand their facts to the analysis.
 *
 * This is the pre-pass the engine will run itself for a step that lists
 * `models: [...]`; calling it from a codemod is how the prototype wires it.
 * Running it again replaces each model's facts.
 */
export function applyModels(root: SgRoot, models: Model[], options: { root?: string } = {}): ApplyResult {
  const all: string[] = root.workspaceFiles().filter((path: string) => !path.includes("/node_modules/"));
  const base = options.root ? options.root.replace(/\/?$/, "/") : commonDirectory(all);
  const result: ApplyResult = { models: [] };
  for (const model of models) {
    const globs = (Array.isArray(model.files) ? model.files : [model.files]).map(globToRegExp);
    const needles = model.contains === undefined ? null : ([] as string[]).concat(model.contains);
    let files = 0;
    let count = 0;
    for (const path of all) {
      const relative = path.startsWith(base) ? path.slice(base.length) : path;
      if (!globs.some((glob) => glob.test(relative))) continue;
      const fileRoot = root.openFile(path);
      if (!fileRoot) continue;
      if (needles) {
        const text: string = fileRoot.root().text();
        if (!needles.some((needle) => text.includes(needle))) {
          root.setModelFacts(model.name, path, "[]");
          continue;
        }
      }
      const facts = runModel(model, fileRoot, { path, relative });
      root.setModelFacts(model.name, path, JSON.stringify(facts));
      files += 1;
      count += facts.length;
    }
    result.models.push({ name: model.name, files, facts: count });
  }
  return result;
}

/** One end of a channel after the join. */
export interface ChannelInfo {
  model: string;
  side: "producer" | "consumer";
  protocol: string;
  key: string;
  node: NodeRef;
  /** The key has an unknown part: it was not joined to anything. */
  open: boolean;
  /** Ends on the other side it was joined to. */
  matches: number;
}

/** Every channel end models declared: which found the other side, which did not. */
export function channels(root: SgRoot): ChannelInfo[] {
  return JSON.parse(root.channels());
}

/**
 * The facts as a manifest another repository's run can load: paths are
 * relative to `base`, so it does not depend on where the checkout is.
 */
export function exportManifest(root: SgRoot, base: string, meta: Record<string, unknown> = {}) {
  const prefix = base.replace(/\/?$/, "/");
  const relative = (path: string) => (path.startsWith(prefix) ? path.slice(prefix.length) : path);
  const facts = JSON.parse(root.modelFacts()) as { model: string; file: string; facts: unknown[] }[];
  const strip = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(strip);
    if (value && typeof value === "object") {
      const object = value as Record<string, unknown>;
      const out: Record<string, unknown> = {};
      for (const [key, inner] of Object.entries(object)) {
        out[key] = key === "file" && typeof inner === "string" ? relative(inner) : strip(inner);
      }
      return out;
    }
    return value;
  };
  return { manifest: 1, ...meta, files: strip(facts) };
}

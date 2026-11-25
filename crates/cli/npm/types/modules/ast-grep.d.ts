declare module "codemod:ast-grep" {
  export class SgNode<M extends TypesMap = TypesMap, out T extends Kinds<M> = Kinds<M>> {
    /** Returns the node's id */
    id(): number;
    range(): Range;
    isLeaf(): boolean;
    isNamed(): boolean;
    isNamedLeaf(): boolean;
    text(): string;
    matches(m: string | number | RuleConfig<M>): boolean;
    inside(m: string | number | RuleConfig<M>): boolean;
    has(m: string | number | RuleConfig<M>): boolean;
    precedes(m: string | number | RuleConfig<M>): boolean;
    follows(m: string | number | RuleConfig<M>): boolean;
    /** Returns the string name of the node kind */
    kind(): T;
    readonly kindToRefine: T;
    /** Check if the node is the same kind as the given `kind` string */
    is<K extends T>(kind: K): this is SgNode<M, K>;
    // we need this override to allow string literal union
    is(kind: string): boolean;

    getMatch: NodeMethod<M, [mv: string]>;
    getMultipleMatches(m: string): Array<SgNode<M>>;
    getTransformed(m: string): string | null;
    /** Returns the node's SgRoot */
    getRoot(): SgRoot<M>;
    children(): Array<SgNode<M>>;
    find: NodeMethod<M, [matcher: string | number | RuleConfig<M>]>;
    findAll<K extends Kinds<M>>(matcher: string | number | RuleConfig<M>): Array<RefineNode<M, K>>;
    /** Finds the first child node in the `field` */
    field<F extends FieldNames<M[T]>>(name: F): FieldNode<M, T, F>;
    /** Finds all the children nodes in the `field` */
    fieldChildren<F extends FieldNames<M[T]>>(name: F): Exclude<FieldNode<M, T, F>, null>[];
    parent: NodeMethod<M>;
    child(nth: number): SgNode<M, ChildKinds<M, T>> | null;
    child<K extends NamedChildKinds<M, T>>(nth: number): RefineNode<M, K> | null;
    ancestors(): Array<SgNode<M>>;
    next: NodeMethod<M>;
    nextAll(): Array<SgNode<M>>;
    prev: NodeMethod<M>;
    prevAll(): Array<SgNode<M>>;
    replace(text: string): Edit;
    commitEdits(edits: Array<Edit>): string;

    // Semantic analysis methods (requires semantic provider)

    /**
     * Get the definition for the symbol at this node's position.
     *
     * Returns an object containing:
     * - `node`: The SgNode at the definition location
     * - `root`: The SgRoot for the file containing the definition
     * - `kind`: The kind of definition ('local', 'import', or 'external')
     *
     * Returns null if:
     * - No semantic provider is configured
     * - No symbol is found at this position
     *
     * When a symbol comes from an import that can't be resolved (e.g., external module),
     * the definition will point to the import statement with `kind: 'import'`.
     *
     * In workspace scope, imported names and names inside a named re-export
     * (`export { a as b } from "./a"`) follow the re-export chain, through
     * any `export *`, to the original declaration (`kind: 'external'`).
     *
     * @param options - Optional settings for definition lookup
     * @param options.resolveExternal - If false, stop at import statements without
     *   attempting to resolve the source module. Default: true
     * @returns An object with the definition node, its root, and kind, or null
     */
    definition(options?: DefinitionOptions): DefinitionResult<M> | null;

    /**
     * The type of this expression or declaration name, as the TypeScript
     * checker prints it (`"UserRepo"`, `"{ id: string; }"`, `"void"`).
     *
     * Needs types to be on: `semanticAnalysis: { mode: "workspace", types:
     * "ts7" }` with TypeScript 7.0 installed in the workspace (or
     * `CODEMOD_TS_API` naming its `tsc`). Null otherwise, in file scope,
     * and when no expression has exactly this node's range. Answers describe
     * the files as they were when the checker first read them.
     * @experimental
     */
    typeInfo(): string | null;

    /**
     * Find all references to the symbol at this node's position.
     *
     * Returns an array of objects, one per file, each containing:
     * - `root`: The SgRoot for the file
     * - `nodes`: Array of SgNode objects for each reference in that file
     *
     * Returns an empty array if:
     * - No semantic provider is configured
     * - No symbol is found at this position
     *
     * In file scope mode, it only searches files that have been processed.
     * In workspace scope mode, it searches the entire workspace.
     *
     * @returns An array of file references, each with a root and nodes
     */
    references(): Array<FileReferences<M>>;

    /**
     * A string naming this node across answers and roots: its file, range and
     * kind. Each semantic query wraps nodes in new objects (and other files in
     * new roots), so `===` and `id()` do not match across queries; keys do.
     * Use it in a `Set` or `Map` to dedupe or remember nodes.
     *
     * @example
     * ```ts
     * const seen = new Set<string>();
     * for (const caller of fn.callers({ transitive: true })) {
     *   if (seen.has(caller.node.key())) continue; // one edit per function
     *   seen.add(caller.node.key());
     * }
     * ```
     */
    key(): string;

    /**
     * Whether `other` is the same syntax node (same file, range and kind),
     * even when it comes from another query or root. Same as comparing `key()`.
     */
    equals(other: SgNode<M>): boolean;

    /**
     * Whether `other` is this node or inside it: same file, and its range
     * within this one's. Works across queries and roots, like `equals()`.
     *
     * @example
     * ```ts
     * // Uses of a name inside this function only
     * const own = name.references().flatMap((f) => f.nodes).filter((n) => fn.contains(n));
     * ```
     */
    contains(other: SgNode<M>): boolean;

    // Data flow and call graph (experimental; JavaScript, TypeScript and Python).
    // Every answer is a "may" answer: see the soundness notes on each method
    // and the result types, and https://docs.codemod.com/community/jssg/dataflow.

    /**
     * Follow this node's value backward: what it is computed from.
     *
     * Steps come nearest first (by `depth`). The answer stays inside the
     * node's file unless `interprocedural` is set, and is computed from the
     * files' current content.
     *
     * Soundness:
     * - `boundary: true` marks where the trail ends because the value comes
     *   from outside the analyzed code: a parameter, an import, a global,
     *   `this`, or a call result. It means "unknown", not "safe".
     * - A `let` assigned in two places has both values as sources
     *   (flow-insensitive). `c ? a : b` has `a` and `b` as sources, not `c`.
     * - Closures do not carry flow: a callback that reads `x` is not
     *   "computed from" `x`.
     * - With `throughCalls` (the default) a call's result counts as derived
     *   from its arguments and receiver, so the trail continues into them.
     * - `truncated` on the result: a limit was hit and steps are missing.
     *
     * @throws when the query cannot run, rather than answering `[]` (which
     *   would read as "nothing found"): semantic analysis is off for the step,
     *   the file is not JavaScript or TypeScript, the root was parsed from a
     *   string, the host is Node.js or WebAssembly, or an option is unknown;
     *   and (a TypeError naming the node's kind) when the query does not apply
     *   to the node: a statement or declaration for `flowsFrom`/`flowsTo`,
     *   anything but a variable or a use of one for `mutations`, code outside
     *   any function for `callers`. An expression the analysis does not track
     *   (in a type position: `typeof import("./x")`) answers `[]` instead.
     *
     * @experimental
     * @example
     * ```ts
     * // Is the SQL text built from request data?
     * const call = root.root().find({ rule: { pattern: "$DB.query($SQL)" } })!;
     * const fromRequest = call
     *   .getMatch("SQL")!
     *   .flowsFrom()
     *   .some((step) => step.kind === "parameter" && step.name === "req");
     * ```
     */
    flowsFrom(options?: FlowOptions): FlowSteps<M>;

    /**
     * Follow this node's value forward: the variables, calls, returns,
     * property writes and JSX attributes it reaches.
     *
     * A step with `via: "argument"` is a call that receives the value, and
     * `index` is the argument position. A step with `via: "return"` or
     * `via: "store"` (`o.p = x`) is reported and not followed further: the
     * value leaves the function or goes into the heap.
     *
     * Soundness: as for `flowsFrom()`; `boundary` is always `false` on
     * forward steps.
     *
     * @throws when the query cannot run, rather than answering `[]` (which
     *   would read as "nothing found"): semantic analysis is off for the step,
     *   the file is not JavaScript or TypeScript, the root was parsed from a
     *   string, the host is Node.js or WebAssembly, or an option is unknown;
     *   and (a TypeError naming the node's kind) when the query does not apply
     *   to the node: a statement or declaration for `flowsFrom`/`flowsTo`,
     *   anything but a variable or a use of one for `mutations`, code outside
     *   any function for `callers`. An expression the analysis does not track
     *   (in a type position: `typeof import("./x")`) answers `[]` instead.
     *
     * @experimental
     * @example
     * ```ts
     * // Every call that receives a value derived from req.body
     * const calls = root.root().findAll({ rule: { pattern: "req.body" } })
     *   .flatMap((body) => body.flowsTo())
     *   .filter((step) => step.via === "argument")
     *   .map((step) => step.node);
     * ```
     */
    flowsTo(options?: FlowOptions): FlowSteps<M>;

    /**
     * Whether this node's value is computed from `source`: `true` when a step
     * of `flowsFrom(options)` is that node, or matches the predicate.
     * Same soundness as `flowsFrom()`: `false` means "not found", and a
     * `boundary` step can hide more sources.
     *
     * @throws as `flowsFrom()` does, and when `source` is neither.
     * @experimental
     * @example
     * ```ts
     * // Is the SQL text built from request data?
     * sql.dependsOn((step) => step.kind === "parameter" && step.name === "req");
     * // Is this argument created by moment(...) directly, not through an alias?
     * name.dependsOn((step) => step.kind === "call" && step.name === "moment", { maxDepth: 1 });
     * ```
     */
    dependsOn(source: SgNode<M> | ((step: FlowStep<M>) => boolean), options?: FlowOptions): boolean;

    /**
     * Whether this node's value reaches `target`: `true` when a step of
     * `flowsTo(options)` is that node, or matches the predicate.
     *
     * @throws as `flowsTo()` does, and when `target` is neither.
     * @experimental
     * @example
     * ```ts
     * // Does req.body reach a db.query(...) call?
     * body.reaches((step) => step.via === "argument" && step.name === "db.query");
     * ```
     */
    reaches(target: SgNode<M> | ((step: FlowStep<M>) => boolean), options?: FlowOptions): boolean;

    /**
     * What the code at this node calls. The answer depends on the node:
     * - the program (`root.root()`): calls made while the module is
     *   evaluated (top-level code), so "does importing this run code?";
     * - a call or a JSX element: what that call calls (for `<Modal />`, the
     *   `Modal` component, `via: "jsx"`);
     * - a function or its name: the calls written in its body;
     * - any other code (a statement, a block): the calls written inside it.
     *
     * Each entry is either a function the call graph resolved
     * (`resolved: true`, `node` is the function, possibly in another file),
     * or a call it could not resolve (`resolved: false`, `node` is the call):
     * a library, a builtin, or a value the analysis lost track of. In file
     * scope, imported functions are unresolved.
     *
     * Soundness:
     * - A function passed to an unresolved call (`items.map(f)`,
     *   `setTimeout(f)`) is assumed to be called: `via: "callback"`.
     * - `unparsed`: imports of visited files that failed to parse; calls
     *   into them are missing. `truncated`: a limit was hit.
     *
     * @throws when the query cannot run, rather than answering `[]` (which
     *   would read as "nothing found"): semantic analysis is off for the step,
     *   the file is not JavaScript or TypeScript, the root was parsed from a
     *   string, the host is Node.js or WebAssembly, or an option is unknown;
     *   and (a TypeError naming the node's kind) when the query does not apply
     *   to the node: a statement or declaration for `flowsFrom`/`flowsTo`,
     *   anything but a variable or a use of one for `mutations`, code outside
     *   any function for `callers`. An expression the analysis does not track
     *   (in a type position: `typeof import("./x")`) answers `[]` instead.
     *
     * @experimental
     * @example
     * ```ts
     * const handler = root.root().find({ rule: { kind: "function_declaration" } })!;
     * for (const callee of handler.callees({ transitive: true })) {
     *   if (callee.resolved) {
     *     callee.node;          // the function, in its own file
     *     callee.callSite.node; // the call that reaches it
     *   } else {
     *     callee.name;          // "Number", "res.json", ...
     *   }
     * }
     * ```
     */
    callees(options?: CallGraphOptions): CallGraphEntries<CalleeEntry<M>>;

    /**
     * Calls of the function at this node (a function, its name, or a call to
     * it), one entry per call. `node` is the calling function, or the call
     * itself when it is made from module top-level code (`name: "<module>"`).
     * Needs workspace scope to see callers in other files.
     *
     * With `transitive: true` a function reached through several paths has
     * one entry per call on those paths.
     *
     * Soundness:
     * - Getters and setters are run by property reads and writes
     *   (`obj.value`, `obj.value = x`), which are not calls: `[]` for a
     *   getter or setter does not mean it is unused.
     * - An empty answer does not mean "unused": a function stored as a value
     *   (`{ round: fn }`, `export default fn`) or called dynamically has no
     *   callers the graph can see. Check `references()` too. Callers are call
     *   sites only: a function a package exports, or a route handler, is
     *   called from outside the workspace. Ask `entryPoints()` for that.
     * - Like every semantic answer it describes the workspace as it was when
     *   the step started: the step's own edits are not in it.
     * - `unparsed`: workspace files that failed to parse; calls in them are
     *   missing. `truncated`: a limit was hit.
     *
     * @throws when the query cannot run, rather than answering `[]` (which
     *   would read as "nothing found"): semantic analysis is off for the step,
     *   the file is not JavaScript or TypeScript, the root was parsed from a
     *   string, the host is Node.js or WebAssembly, or an option is unknown;
     *   and (a TypeError naming the node's kind) when the query does not apply
     *   to the node: a statement or declaration for `flowsFrom`/`flowsTo`,
     *   anything but a variable or a use of one for `mutations`, code outside
     *   any function for `callers`. An expression the analysis does not track
     *   (in a type position: `typeof import("./x")`) answers `[]` instead.
     *
     * @experimental
     * @example
     * ```ts
     * // Add await to every call of loadConfig
     * const edits = loadConfig.callers().map((caller) =>
     *   caller.callSite.node.replace(`await ${caller.callSite.node.text()}`),
     * );
     * ```
     */
    callers(options?: CallGraphOptions): CallGraphEntries<CallerEntry<M>>;

    /**
     * Why the function at this node is called from outside the workspace's
     * code: its package exports it through `exports` or `main`
     * (`{ kind: "export", label: "@acme/ui#Button" }`), or a rule in the
     * selector's `semanticAnalysis.entryPoints` declares it
     * (`{ kind: "http", label: "GET app/api/users/[id]/route.ts" }`).
     * `[]` when neither applies. `callers()` never includes these.
     * Needs workspace scope.
     *
     * @experimental
     */
    entryPoints(): EntryPoint[];

    /**
     * Places that change the variable at this node, or the object it holds:
     * reassignments, `++`/`--`, property writes, `delete`, mutating method
     * calls, and calls it is passed to. Includes changes made through other
     * variables holding the same value (`viaAlias`) and inside nested
     * functions (`deferred`). Stays inside the node's file.
     *
     * Soundness:
     * - `confidence`: `"definite"` is a write in the syntax, `"likely"` a
     *   method known to mutate (`push`, `set`, ...), `"maybe"` a call the
     *   analysis has no summary for (it may or may not change the value).
     * - `after` uses text order plus "same loop", and counts every deferred
     *   mutation as "after": a closure can run at any time.
     *
     * @throws when the query cannot run, rather than answering `[]` (which
     *   would read as "nothing found"): semantic analysis is off for the step,
     *   the file is not JavaScript or TypeScript, the root was parsed from a
     *   string, the host is Node.js or WebAssembly, or an option is unknown;
     *   and (a TypeError naming the node's kind) when the query does not apply
     *   to the node: a statement or declaration for `flowsFrom`/`flowsTo`,
     *   anything but a variable or a use of one for `mutations`, code outside
     *   any function for `callers`. An expression the analysis does not track
     *   (in a type position: `typeof import("./x")`) answers `[]` instead.
     *
     * @experimental
     * @example
     * ```ts
     * // Is `items` changed after the audit() call?
     * const changed = items
     *   .mutations({ after: auditCall })
     *   .some((m) => m.confidence !== "maybe");
     * ```
     */
    mutations(options?: MutationOptions<M>): Array<Mutation<M>>;
    /**
     * The type of this expression as the TypeScript 7 checker prints it
     * (`"Promise<User>"`), or `null` when there is no checker or no answer.
     * Needs `semanticAnalysis: "workspace"` and a TypeScript 7 checker: the
     * target's `node_modules/@typescript/native-preview-<os>-<arch>` (or
     * `typescript-<os>-<arch>`), or `CODEMOD_TS_API=<path to tsc/tsgo>`.
     *
     * @experimental
     */
    typeInfo(): string | null;
  }
  /** Represents the parsed tree of code. */
  export class SgRoot<M extends TypesMap = TypesMap> {
    /** Returns the root SgNode of the ast-grep instance. */
    root(): SgNode<M, RootKind<M>>;
    /**
     * Returns the path of the file if it is discovered by ast-grep's `findInFiles`.
     * Returns `"anonymous"` if the instance is created by `lang.parse(source)`.
     */
    filename(): string;
    /**
     * Returns the file path relative to the current target directory.
     */
    relativeFilename(): string;
    /**
     * Returns the source code of the file.
     */
    source(): string;
    /**
     * Write content to this file.
     *
     * This method is only valid for files obtained via `definition()` or `references()`.
     * It cannot be called on the current file being processed - for that, return the
     * modified content from the `transform()` function instead.
     *
     * After writing, the semantic provider's cache is automatically updated with the new content.
     *
     * @param content - The new content to write to the file
     * @throws Error if called on the current file being processed
     * @throws Error if the file has no path
     * @throws Error if the write operation fails
     *
     * @example
     * ```ts
     * export default function transform(root) {
     *   const node = root.root().find({ rule: { pattern: "myVar" } });
     *   const def = node.definition();
     *
     *   if (def && def.root.filename() !== root.filename()) {
     *     // Edit the other file
     *     const edits = [def.node.replace("newVar")];
     *     const newContent = def.root.root().commitEdits(edits);
     *     def.root.write(newContent);
     *   }
     *
     *   // Return changes for the current file
     *   return root.root().commitEdits([...]);
     * }
     * ```
     */
    write(content: string): void;

    // Module resolution (experimental; JavaScript and TypeScript, workspace scope).

    /**
     * The file `specifier` resolves to when this root's file imports it, as
     * `definition()` resolves it: tsconfig `paths` (with `extends`), workspace
     * packages, `node_modules`, index files and extensions.
     *
     * @throws when semantic analysis is not in workspace scope, or the root
     *   was parsed from a string.
     * @returns The absolute path, or null when it does not resolve.
     * @experimental
     * @example
     * ```ts
     * root.resolveModule("@/components"); // "/repo/src/components/index.ts"
     * ```
     */
    resolveModule(specifier: string): string | null;

    /**
     * What export `name` (`"default"` for the default export) of `module`
     * lands on, following re-exports through barrels and `export *`.
     * `module` is a specifier, resolved from this root's file, or an
     * absolute path (from `resolveModule()`).
     *
     * @throws as `resolveModule()` does.
     * @returns A `definition()`-shaped result (`exportName` is how the
     *   declaring module exports it), or null when the module does not
     *   export the name or does not resolve.
     * @experimental
     * @example
     * ```ts
     * const modal = root.resolveExport("@/components", "Modal");
     * modal?.root.filename(); // ".../components/modal.tsx"
     * modal?.exportName;      // "default"
     * ```
     */
    resolveExport(module: string, name: string): DefinitionResult<M> | null;

    /**
     * A specifier this root's file can import `target` with. Candidates are
     * the relative path and every tsconfig `paths` alias whose target holds
     * the file; each is checked by resolving it, so the answer always
     * reaches `target`. With `like` (the specifier being replaced) its style
     * wins: relative or alias, the same alias prefix, `.js` or no extension.
     * Otherwise `prefer`, then the shortest.
     *
     * @param target - An absolute path, a path relative to this file, or an
     *   SgRoot (from `definition()` or `resolveExport()`).
     * @throws as `resolveModule()` does, and on an unknown option.
     * @returns The specifier, or null when none reaches `target`.
     * @experimental
     * @example
     * ```ts
     * root.specifierFor(modal.root, { like: "@/components" }); // "@/components/modal"
     * root.specifierFor(modal.root, { like: "../components" }); // "../components/modal"
     * ```
     */
    specifierFor(target: string | SgRoot<M>, options?: SpecifierOptions): string | null;

    /**
     * Every place in the workspace that refers to `target` (this root's file
     * when omitted; a path relative to this file, an absolute path, or an
     * SgRoot): `import` statements (side-effect imports too), `export … from`,
     * `require()`, `import()`, and test-framework mocks (`jest.mock`,
     * `jest.requireActual`, `vi.mock`, `vi.importActual`, ...). Each
     * specifier is resolved from its file, so aliases and barrels count.
     *
     * `import` lines in `.mdx` / `.md` files count too (kind `"import"`).
     *
     * Answers from a table of every module reference in the workspace, built
     * on the first `importers()` query of the run and kept: a snapshot, as
     * for `callers()`. A file rewritten later in the run still counts as
     * importing what it imported then, so a barrel's consumers are known
     * even after they were pointed elsewhere. `unparsed` lists files that
     * did not parse: references in them are missing.
     *
     * @throws as `resolveModule()` does.
     * @experimental
     * @example
     * ```ts
     * // Can this barrel be deleted? Not while anything still imports it.
     * const left = root.importers();
     * if (left.length === 0 && left.unparsed.length === 0) {
     *   // no importers in the workspace
     * }
     * ```
     */
    /**
     * The node a `node.key()` names, in this root; null when the key names
     * another file, or this file has no node of that range and kind any more.
     * Keys are plain strings, so a step can return them (in `forEachFile`
     * outputs, JSON) and a later step can turn them back into nodes.
     *
     * @throws a TypeError when `key` is not a `node.key()`.
     * @example
     * ```ts
     * // Step 1 collects keys; step 2, in each file:
     * const call = parse(file).nodeByKey(plan.key);
     * if (call) file.write(parse(file).root().commitEdits([call.replace("fixed()")]));
     * ```
     */
    nodeByKey(key: string): SgNode<M> | null;

    importers(target?: string | SgRoot<M>): Array<ImporterEntry<M>> & { unparsed: UnparsedFile[] };

    /**
     * Whether importing this module runs code: a call made at module top
     * level (what `root.root().callees()` answers), an `import "./x"`
     * side-effect import, or a top-level write to an object member
     * (`registry.x = …`). Declarations alone are not effects. A "may" answer:
     * an unresolved call counts even if it is pure, and a side-effect import
     * counts without looking at the imported module.
     *
     * @throws as `resolveModule()` does.
     * @experimental
     * @example
     * ```ts
     * // Skipping a barrel also skips its side effects: keep it when it has some.
     * if (barrel.hasTopLevelEffects()) return null;
     * ```
     */
    hasTopLevelEffects(): boolean;

    /**
     * The specifiers another package can import this file with: its
     * package's name (when it is the package's entry) and the `exports`
     * subpaths that resolve to it. Empty when the file is not part of its
     * package's public API, or not in a named package. Each is checked by
     * resolving it; a file named by `main` / `module` / `types` / `source` in
     * a package without `exports` is public by declaration, even when the
     * name does not resolve (no install, no workspace config).
     *
     * @throws as `resolveModule()` does.
     * @experimental
     * @example
     * ```ts
     * // A barrel that is its package's entry point cannot be removed.
     * if (barrel.publicSpecifiers().length > 0) return null;
     * ```
     */
    publicSpecifiers(): string[];
    /**
     * Rename the current file to a new path.
     *
     * If the path is relative, it is resolved against the current file's parent directory.
     * If absolute, it is used as-is.
     *
     * The rename is applied after the transform function returns:
     * - If the transform returns a string, the file is renamed and its content is updated.
     * - If the transform returns null, the file is renamed with its original content.
     *
     * @param newPath - The new file path (relative or absolute)
     * @throws Error if the path is empty
     *
     * @example
     * ```ts
     * // Rename .less to .css
     * export default function transform(root) {
     *   root.rename(root.filename().replace('.less', '.css'));
     *   return transformedContent;
     * }
     * ```
     */
    rename(newPath: string): void;
  }

  interface NodeMethod<M extends TypesMap, Args extends unknown[] = []> {
    (...args: Args): SgNode<M> | null;
    <K extends NamedKinds<M>>(...args: Args): RefineNode<M, K> | null;
  }

  /**
   * if K contains string, return general SgNode. Otherwise,
   * if K is a literal union, return a union of SgNode of each kind.
   */
  type RefineNode<M extends TypesMap, K> = string extends K
    ? SgNode<M>
    : K extends Kinds<M>
      ? SgNode<M, K>
      : never;

  /**
   * return the SgNode of the field in the node.
   */
  // F extends string is used to prevent noisy TS hover info
  type FieldNode<
    M extends TypesMap,
    K extends Kinds<M>,
    F extends FieldNames<M[K]>,
  > = F extends string ? FieldNodeImpl<M, ExtractField<M[K], F>> : never;

  type FieldNodeImpl<M extends TypesMap, I extends NodeFieldInfo> = I extends {
    required: true;
  }
    ? RefineNode<M, TypesInField<M, I>>
    : RefineNode<M, TypesInField<M, I>> | null;

  /**
   * Rule configuration similar to YAML
   * See https://ast-grep.github.io/reference/yaml.html
   */
  export interface RuleConfig<M extends TypesMap = TypesMap> {
    /** The rule object, see https://ast-grep.github.io/reference/rule.html */
    rule: Rule<M>;
    /** See https://ast-grep.github.io/guide/rule-config.html#constraints */
    constraints?: Record<string, Rule<M>>;
    /** Builtin Language or custom language */
    language?: string;
    /**
     * transform is NOT useful in JavaScript. You can use JS code to directly transform the result.
     * https://ast-grep.github.io/reference/yaml.html#transform
     */
    transform?: unknown;
    /** https://ast-grep.github.io/guide/rule-config/utility-rule.html */
    utils?: Record<string, Rule<M>>;
  }

  export interface FileOption {
    paths: Array<string>;
    languageGlobs: Record<string, Array<string>>;
  }

  export interface FindConfig<M extends TypesMap = TypesMap> {
    /** specify the file paths to recursively find files */
    paths: Array<string>;
    /** a Rule object to find what nodes will match */
    matcher: RuleConfig<M>;
    /**
     * An list of pattern globs to treat of certain files in the specified language.
     * eg. ['*.vue', '*.svelte'] for html.findFiles, or ['*.ts'] for tsx.findFiles.
     * It is slightly different from https://ast-grep.github.io/reference/sgconfig.html#languageglobs
     */
    languageGlobs?: Array<string>;
  }

  export type Strictness = "cst" | "smart" | "ast" | "relaxed" | "signature";

  export interface PatternObject<M extends TypesMap = TypesMap> {
    context: string;
    selector?: NamedKinds<M>; // only named node types
    strictness?: Strictness;
  }

  export type PatternStyle<M extends TypesMap = TypesMap> = string | PatternObject<M>;

  export interface Relation<M extends TypesMap = TypesMap> extends Rule<M> {
    /**
     * Specify how relational rule will stop relative to the target node.
     */
    stopBy?: "neighbor" | "end" | Rule<M>;
    /** Specify the tree-sitter field in parent node. Only available in has/inside rule. */
    field?: string;
  }

  export interface NthChildObject<M extends TypesMap = TypesMap> {
    /** The position in nodes' sibling list. It can be a number of An+B string */
    position: string | number;
    ofRule?: Rule<M>;
    reverse?: boolean;
  }

  /**
   * NthChild can have these types:
   * * number: the position of the node in the sibling list.
   * * string: An + B style string like CSS nth-child selector.
   * * object: An object with `position` and `ofRule` fields.
   */
  export type NthChild<M extends TypesMap = TypesMap> = number | string | NthChildObject<M>;

  export interface Rule<M extends TypesMap = TypesMap> {
    /** A pattern string or a pattern object. */
    pattern?: PatternStyle<M>;
    /** The kind name of the node to match. You can look up code's kind names in playground. */
    kind?: NamedKinds<M>;
    /** The exact range of the node in the source code. */
    range?: Range;
    /** A Rust regular expression to match the node's text. https://docs.rs/regex/latest/regex/#syntax */
    regex?: string;
    /**
     * `nthChild` accepts number, string or object.
     * It specifies the position in nodes' sibling list. */
    nthChild?: NthChild<M>;

    // relational
    /**
     * `inside` accepts a relational rule object.
     * the target node must appear inside of another node matching the `inside` sub-rule. */
    inside?: Relation<M>;
    /**
     * `has` accepts a relational rule object.
     * the target node must has a descendant node matching the `has` sub-rule. */
    has?: Relation<M>;
    /**
     * `precedes` accepts a relational rule object.
     * the target node must appear before another node matching the `precedes` sub-rule. */
    precedes?: Relation<M>;
    /**
     * `follows` accepts a relational rule object.
     * the target node must appear after another node matching the `follows` sub-rule. */
    follows?: Relation<M>;
    // composite
    /**
     * A list of sub rules and matches a node if all of sub rules match.
     * The meta variables of the matched node contain all variables from the sub-rules. */
    all?: Array<Rule<M>>;
    /**
     * A list of sub rules and matches a node if any of sub rules match.
     * The meta variables of the matched node only contain those of the matched sub-rule. */
    any?: Array<Rule<M>>;
    /** A single sub-rule and matches a node if the sub rule does not match. */
    not?: Rule<M>;
    /** A utility rule id and matches a node if the utility rule matches. */
    matches?: string;
  }

  type Separator = "caseChange" | "dash" | "dot" | "slash" | "space" | "underscore";

  type StringCase =
    | "lowerCase"
    | "upperCase"
    | "capitalize"
    | "camelCase"
    | "snakeCase"
    | "kebabCase"
    | "pascalCase";

  interface Substring {
    endChar?: number | null;
    source: string;
    startChar?: number | null;
  }

  interface Replace {
    by: string;
    replace: string;
    source: string;
  }

  type Transformation =
    | { substring: Substring }
    | { replace: Replace }
    | { convert: Convert }
    | { rewrite: Rewrite };

  interface Rewrite {
    joinBy?: string | null;
    rewriters: string[];
    source: string;
  }

  interface SerializableFixConfig {
    expandEnd?: Relation;
    expandStart?: Relation;
    template: string;
  }

  type SerializableFixer = string | SerializableFixConfig;

  export type Severity = "hint" | "info" | "warning" | "error" | "off";

  export interface CompleteRuleConfig<M extends TypesMap = TypesMap> extends RuleConfig<M> {
    id: string;
    fix?: SerializableFixer;
    transform?: Record<string, Transformation>;
    severity?: Severity;
    note?: string;
    message?: string;
    ignores?: string[];
    url?: string;
  }

  // Definitions
  interface Convert {
    separatedBy?: Separator[] | null;
    source: string;
    toCase: StringCase;
  }

  export interface Edit {
    /** The start position of the edit */
    startPos: number;
    /** The end position of the edit */
    endPos: number;
    /** The text to be inserted */
    insertedText: string;
  }
  export interface Position {
    /** line number starting from 0 */
    line: number;
    /** column number starting from 0 */
    column: number;
    /** byte offset of the position */
    index: number;
  }
  export interface Range {
    /** starting position of the range */
    start: Position;
    /** ending position of the range */
    end: Position;
  }

  /**
   * Result of getDefinition() - contains the definition node and its root.
   */
  /**
   * The kind of definition that was found.
   * - 'local': Definition is in the same file (local variable, function, class, etc.)
   * - 'import': Definition traced to an import statement but module couldn't be resolved
   * - 'external': Definition resolved to a different file in the workspace
   */
  export type DefinitionKind = "local" | "import" | "external";

  /**
   * Options for definition lookup.
   */
  export interface DefinitionOptions {
    /**
     * If true, attempt to resolve imports to their source modules.
     * If false, stop at the import statement.
     * @default true
     */
    resolveExternal?: boolean;
    /**
     * Go through import bindings: a use of an imported name normally lands
     * on its binding in the import statement (`kind: 'local'`); with
     * `follow` the lookup continues from there to the declaration in the
     * other module (`kind: 'external'`), or to the import when the module
     * cannot be resolved (`kind: 'import'`).
     * @default false
     * @example
     * ```ts
     * // <Modal /> → export default function Modal() in ./modal.tsx
     * const def = tag.definition({ follow: true });
     * def?.exportName; // "default"
     * def?.nameNode?.text(); // "Modal"
     * ```
     */
    follow?: boolean;
  }

  /** One place that refers to a module, from `root.importers()`. */
  export interface ImporterEntry<M extends TypesMap = TypesMap> {
    /**
     * The `import` or `export … from` statement, or the `require(…)`,
     * `import(…)` or mock call.
     */
    node: SgNode<M>;
    root: SgRoot<M>;
    /**
     * - 'import': `import … from "m"`, `import "m"`
     * - 'export': `export … from "m"`, `export * from "m"`
     * - 'require': `require("m")`
     * - 'dynamic': `import("m")`
     * - 'mock': `jest.mock("m")`, `jest.requireActual("m")`, `vi.mock("m")`, `vi.importActual("m")`, ...
     */
    kind: "import" | "export" | "require" | "dynamic" | "mock";
    /** The specifier as written. */
    specifier: string;
  }

  export interface SpecifierOptions {
    /**
     * The specifier being replaced: its style (relative or alias), alias
     * prefix and extension are kept when a specifier like it reaches the file.
     * Takes precedence over `prefer`.
     */
    like?: string;
    /** Which kind to choose when both reach the file. */
    prefer?: "relative" | "alias";
  }

  export interface DefinitionResult<M extends TypesMap = TypesMap> {
    /**
     * The SgNode at the definition location. Depending on the declaration
     * this is the name, the whole declarator (`LIMIT: number = 10`) or the
     * whole `export default function ...` statement; use `nameNode` for the name.
     */
    node: SgNode<M>;
    /** The SgRoot for the file containing the definition */
    root: SgRoot<M>;
    /** The kind of definition (local, import, or external) */
    kind: DefinitionKind;
    /**
     * The declared name's identifier inside `node` (`format` in
     * `function format() {}`, `Modal` in `export default function Modal`).
     * Null for an anonymous declaration.
     */
    nameNode: SgNode<M> | null;
    /**
     * The name the declaration is exported under from its module:
     * `"default"` for a default export, `"formatValue"` for
     * `export { format as formatValue }`. Null when it is not exported.
     * A declaration exported under several names reports one of them.
     */
    exportName: string | null;
  }

  /**
   * References in a single file - contains the root and all reference nodes.
   */
  export interface FileReferences<M extends TypesMap = TypesMap> {
    /** The SgRoot for the file */
    root: SgRoot<M>;
    /** Array of SgNode objects for each reference in this file */
    nodes: Array<SgNode<M>>;
  }

  // ---- Data flow ----------------------------------------------------------

  /**
   * What a step of a data-flow answer is.
   * - 'variable': a local binding (`const`, `let`, `var`, function or class name)
   * - 'parameter': a function parameter; the value comes from the callers (boundary)
   * - 'import': an imported binding (boundary)
   * - 'reference': a read of a binding
   * - 'function' / 'class': a function, arrow function or class value
   * - 'call': a call or `new`; its result is a boundary
   * - 'member': a property read (`a.b`, `a[b]`) or a property write's target
   * - 'literal', 'object', 'array', 'jsx', 'expression': values built in place
   * - 'global': a name with no binding in the module (boundary)
   * - 'this': `this` (boundary)
   * - 'jsx-attribute': a JSX attribute
   */
  export type FlowNodeKind =
    | "variable"
    | "parameter"
    | "import"
    | "reference"
    | "function"
    | "class"
    | "call"
    | "member"
    | "literal"
    | "global"
    | "this"
    | "object"
    | "array"
    | "jsx"
    | "jsx-attribute"
    | "expression";

  /**
   * How a value gets from one step to the next.
   * - 'assign': initializer or assignment into a variable
   * - 'read': a variable read by a reference
   * - 'alias': the same value under another expression (`(x)`, `a ?? b`, `c ? a : b`)
   * - 'property' / 'element': part of the value (`x.p`, `const { p } = x`, `x[i]`)
   * - 'argument' / 'receiver': the value is passed to, or is the receiver of, a call
   * - 'derive': computed from the value (`x + 1`, a template literal, `await x`)
   * - 'capture': stored in a new object or array (`{ p: x }`, `[x]`)
   * - 'store': written to a property (`o.p = x`); forward answers stop here
   * - 'jsx-attribute' / 'jsx-child': the value is passed to a JSX element
   * - 'return': the value is returned from the function; forward answers stop here
   */
  export type FlowEdgeKind =
    | "assign"
    | "read"
    | "alias"
    | "property"
    | "element"
    | "argument"
    | "receiver"
    | "derive"
    | "capture"
    | "store"
    | "jsx-attribute"
    | "jsx-child"
    | "return";

  export interface FlowOptions {
    /**
     * Treat a call's result as derived from its arguments and receiver, and
     * keep following the value through them. This is the safe assumption for
     * calls the analysis cannot see into. With `false`, every call is a stop.
     * @default true
     */
    throughCalls?: boolean;
    /** Stop after this many steps from the node. `1` gives the direct sources (or uses) only. */
    maxDepth?: number;
    /**
     * Cross function boundaries inside the workspace. Backward, a parameter
     * continues at its callers' arguments and a resolved call's result at the
     * callee's return values; forward, an argument goes into the callee's
     * parameter and a returned value out to the call sites. A value that
     * enters a function through a call leaves it through that same call.
     * Answers then span files (`step.root` differs) and can be much larger:
     * combine with `maxDepth` or `stopAt`. Callback edges are not followed.
     * Uses the workspace callers table (a snapshot, as for `callers()`).
     * @default false
     * @example
     * ```ts
     * // Does a request value reach query() through helper functions?
     * sql.dependsOn((s) => s.kind === "member" && s.name === "body", { interprocedural: true });
     * ```
     */
    interprocedural?: boolean;
    /**
     * Report steps reached through these edges, but do not walk past them:
     * "the first sink". `flowsTo({ stopAt: ["jsx-child", "jsx-attribute", "argument"] })`
     * answers where a value is first used, not every place its uses go on to.
     */
    stopAt?: FlowEdgeKind[];
  }

  /** Fields every data-flow step has. */
  interface FlowStepBase<M extends TypesMap> {
    /** The syntax node of this step, in `root`. */
    node: SgNode<M>;
    root: SgRoot<M>;
    /** Number of edges from the queried node (1 = a direct source or use). */
    depth: number;
    /**
     * The step this one was reached from, or null for a step reached directly
     * from the queried node (`depth` 1). Follow `from` to print the whole
     * path, or to find the step right before one, without counting depths.
     * With `interprocedural`, `from` can be a step in another file.
     *
     * @example
     * ```ts
     * // The value a variable passed to query() was initialized with
     * const init = sql.flowsFrom().find((s) => s.via === "assign" && s.from?.kind === "variable");
     * ```
     */
    from: FlowStep<M> | null;
    /**
     * Backward answers only: the value comes from outside the analyzed code
     * (a parameter, an import, a global, `this`, a call result) and the trail
     * ends here. "Unknown", not "safe". Always false on forward steps.
     */
    boundary: boolean;
  }

  /** The `kind` of a step, with the fields that kind always has. */
  type FlowStepByKind =
    | {
        kind: "parameter";
        /** The parameter's name. */
        name: string;
        /** The parameter's position. */
        index: number;
      }
    | {
        kind: "variable" | "import" | "reference" | "global" | "jsx-attribute";
        /** The variable, imported, referenced, global or attribute name. */
        name: string;
        index: number | null;
      }
    | {
        kind: "call";
        /** The callee's text: `"moment"`, `"db.query"`, `"new Date"` is `"Date"`. */
        name: string;
        index: number | null;
      }
    | {
        kind: "member" | "function" | "class" | "expression";
        /** Property name, function or class name, or object key; null when computed or anonymous. */
        name: string | null;
        index: number | null;
      }
    | {
        kind: "literal" | "this" | "object" | "array" | "jsx";
        name: null;
        index: number | null;
      };

  /** The `via` of a step, with the fields that edge always sets. */
  type FlowStepByVia =
    | {
        /** The value is an argument of a call. Forward: `node` is the call. */
        via: "argument";
        /** The argument position. */
        index: number;
      }
    | {
        via: Exclude<FlowEdgeKind, "argument">;
      };

  /**
   * One node reached by `flowsFrom()` or `flowsTo()`.
   *
   * Narrow on `kind` for `name` (always a string for parameters, variables,
   * imports, references, globals and calls) and on `via` for `index`
   * (always the position for `"argument"`):
   *
   * ```ts
   * for (const step of node.flowsFrom()) {
   *   if (step.kind === "parameter") step.name; // string
   *   if (step.via === "argument") step.index;  // number
   * }
   * ```
   */
  export type FlowStep<M extends TypesMap = TypesMap> = FlowStepBase<M> &
    FlowStepByKind &
    FlowStepByVia;

  /**
   * Steps of a data-flow query, nearest first.
   *
   * `truncated` is true when a result limit (50,000 steps) was hit and
   * steps are missing.
   */
  export type FlowSteps<M extends TypesMap = TypesMap> = Array<FlowStep<M>> & {
    truncated: boolean;
  };

  // ---- Call graph ---------------------------------------------------------

  /** Why a function is called from outside the workspace's code. */
  export interface EntryPoint {
    /** `"export"` for the package's public API, else the rule's `kind`. */
    kind: string;
    /** What reaches it: `@acme/ui#Button`, `GET app/api/users/[id]/route.ts`. */
    label: string;
  }

  export interface CallGraphOptions {
    /** Follow calls of calls (callees of callees, callers of callers). @default false */
    transitive?: boolean;
    /** With `transitive`, stop after this many calls from the node. */
    maxDepth?: number;
    /**
     * With `transitive`, report edges of these kinds but do not walk past
     * them. `callers({ transitive: true, stopAt: ["callback"] })` does not
     * continue from a function that only passes the queried one to unknown
     * code (`names.map(getSetting)`).
     */
    stopAt?: CallEdgeKind[];
  }

  /**
   * Why the call graph has an edge.
   * - 'direct': `f()`
   * - 'method': `o.m()`, `this.m()`, `ns.f()`
   * - 'constructor': `new C()`
   * - 'callback': the function is passed to a call the analysis cannot see
   *   into (`items.map(f)`, `setTimeout(f)`) and is assumed to be called
   * - 'jsx': `<Component />`
   * - 'type': `x.m()` resolved by the type of `x` (TypeScript checker), when
   *   the values alone did not resolve it. For an interface member: the
   *   class members that implement it. Experimental.
   */
  export type CallEdgeKind = "direct" | "method" | "constructor" | "callback" | "jsx" | "type";

  /** A call, with the root of its file. */
  export interface CallSite<M extends TypesMap = TypesMap> {
    /** The call expression (or `new`, or JSX element). */
    node: SgNode<M>;
    root: SgRoot<M>;
  }

  /** A function `callees()` resolved. */
  export interface ResolvedCallee<M extends TypesMap = TypesMap> {
    resolved: true;
    /** The function (declaration, expression, arrow or method), possibly in another file. */
    node: SgNode<M>;
    root: SgRoot<M>;
    /** The function's name, or the variable or key it is assigned to; null when anonymous. */
    name: string | null;
    via: CallEdgeKind;
    /** Number of calls from the queried function (1 = called directly). */
    depth: number;
    /** The call that reaches this function. */
    callSite: CallSite<M>;
  }

  /**
   * A call `callees()` could not resolve to a function in the workspace: a
   * library, a builtin, a dynamic call, or (in file scope) an import.
   */
  export interface UnresolvedCallee<M extends TypesMap = TypesMap> {
    resolved: false;
    /** The call itself. */
    node: SgNode<M>;
    root: SgRoot<M>;
    /** The callee's text: `"Number"`, `"res.json"`, `"items.forEach"`. */
    name: string;
    via: Exclude<CallEdgeKind, "callback">;
    depth: number;
    /** The same call as `node`. */
    callSite: CallSite<M>;
    /**
     * With types on (`semanticAnalysis: { mode: "workspace", types: "ts7" }`):
     * where the TypeScript checker says the called member is declared, when
     * that is not a function in the workspace. For example the
     * `isOpen: Accessor<boolean>` property of a context interface, in a
     * `.d.ts` or the workspace. Absent without types or when unknown.
     * @experimental
     */
    declaration?: { node: SgNode<M>; root: SgRoot<M>; name: string | null };
  }

  /** One entry of a `callees()` answer: narrow on `resolved`. */
  export type CalleeEntry<M extends TypesMap = TypesMap> = ResolvedCallee<M> | UnresolvedCallee<M>;

  /** One call of the queried function, found by `callers()`. */
  export interface CallerEntry<M extends TypesMap = TypesMap> {
    /** Always true: a caller is found by resolving its call. */
    resolved: true;
    /**
     * The calling function, or the call itself when it is made from module
     * top-level code.
     */
    node: SgNode<M>;
    root: SgRoot<M>;
    /** The calling function's name, `"<anonymous>"`, or `"<module>"` for top-level code. */
    name: string;
    /** How the call reaches the queried function (`"callback"`: it is passed to unknown code). */
    via: CallEdgeKind;
    /** 1 for a direct caller, 2 for a caller of a caller, ... */
    depth: number;
    /** The call, in the calling function. For a callback, the call it is passed to. */
    callSite: CallSite<M>;
    /**
     * The function this entry's call reaches: the queried function at depth
     * 1, a caller from the previous depth deeper. Null when the call reaches
     * module code.
     */
    callee: { node: SgNode<M>; root: SgRoot<M>; name: string | null } | null;
  }

  /** An entry of either call-graph answer. */
  export type CallGraphEntry<M extends TypesMap = TypesMap> = CalleeEntry<M> | CallerEntry<M>;

  /** A file the analysis could not parse. */
  export interface UnparsedFile {
    path: string;
    /** The parser's message. */
    message: string;
  }

  /**
   * Entries of a call-graph query, nearest first.
   * - `truncated`: a result limit (50,000 entries) was hit and entries are missing
   * - `unparsed`: files the walk needed but could not parse, so calls into
   *   or out of them are missing: imports of the visited functions'
   *   files (`callees`), or workspace files (`callers`)
   *
   * The answer is complete only when `!truncated && unparsed.length === 0`.
   */
  export type CallGraphEntries<E> = Array<E> & {
    truncated: boolean;
    unparsed: UnparsedFile[];
  };

  // ---- Mutations ----------------------------------------------------------

  export interface MutationOptions<M extends TypesMap = TypesMap> {
    /**
     * Only mutations that can run after this node (same file). Uses text
     * order, plus "inside the same loop", plus every deferred mutation.
     */
    after?: SgNode<M>;
  }

  /**
   * How a variable, or the value it holds, is changed.
   * - 'reassign': `x = ...`
   * - 'update': `x++`, `x.p--`
   * - 'property-write': `x.p = ...`
   * - 'delete': `delete x.p`
   * - 'method-call': `x.m(...)`
   * - 'escape': `f(x)`, the callee may change it
   */
  export type MutationKind =
    | "reassign"
    | "update"
    | "property-write"
    | "delete"
    | "method-call"
    | "escape";

  /**
   * How sure the analysis is that a mutation happens.
   * - 'definite': a write in the syntax
   * - 'likely': a method known to mutate its receiver (`push`, `set`, ...)
   * - 'maybe': a call the analysis has no summary for
   */
  export type MutationConfidence = "definite" | "likely" | "maybe";

  /** One place that changes a variable or its value, found by `mutations()`. */
  export interface Mutation<M extends TypesMap = TypesMap> {
    /** The write, update, `delete`, or call (`alias.sort(byPrice)`). */
    node: SgNode<M>;
    root: SgRoot<M>;
    kind: MutationKind;
    confidence: MutationConfidence;
    /** The property or method name (`"sort"`, `"length"`); null for `reassign`, `escape` and `x++`. */
    name: string | null;
    /** In a nested function: runs whenever that function is called, not where it is written. */
    deferred: boolean;
    /** Made through another variable that holds the same value (`const alias = x; alias.sort()`). */
    viaAlias: boolean;
  }

  /**
   * Reference
   * https://tree-sitter.github.io/tree-sitter/using-parsers#static-node-types
   * Rust CLI Impl
   * https://github.com/tree-sitter/tree-sitter/blob/f279d10aa2aca37c0004d84b2261685739f3cab8/cli/generate/src/node_types.rs#L35-L47
   */

  export interface NodeBasicInfo {
    type: string;
    named: boolean;
  }

  export interface NodeFieldInfo {
    multiple: boolean;
    required: boolean;
    types: NodeBasicInfo[];
  }

  export interface NodeType extends NodeBasicInfo {
    root?: boolean;
    fields?: {
      [fieldName: string]: NodeFieldInfo;
    };
    children?: NodeFieldInfo;
    subtypes?: NodeBasicInfo[];
  }

  /**
   * A map of key to NodeType.
   * Note, the key is not necessary node's kind.
   * it can be a rule representing a category of syntax nodes
   * (e.g. “expression”, “type”, “declaration”).
   * See reference above for more details.
   */
  export interface TypesMap {
    [key: string]: NodeType;
  }

  export type FieldNames<N extends NodeType> = N extends { fields: infer F } ? keyof F : string;

  export type ExtractField<N extends NodeType, F extends FieldNames<N>> =
    N["fields"] extends Record<F, NodeFieldInfo> ? N["fields"][F] : NodeFieldInfo;

  // in case of empty types array, return string as fallback
  type NoNever<T, Fallback> = [T] extends [never] ? Fallback : T;

  export type TypesInField<M extends TypesMap, I extends NodeFieldInfo> = NoNever<
    ResolveType<M, I["types"][number]["type"]>,
    Kinds<M>
  >;

  export type NamedChildKinds<M extends TypesMap, T extends Kinds<M>> = M[T] extends {
    children: infer C extends NodeFieldInfo;
  }
    ? TypesInField<M, C>
    : NamedKinds<M>;
  export type ChildKinds<M extends TypesMap, T extends Kinds<M>> =
    | NamedChildKinds<M, T>
    | LowPriorityKey;

  /**
   * resolve subtypes alias. see tree-sitter's reference
   * e.g. like `expression` => `binary_expression` | `unary_expression` | ...
   */
  type ResolveType<M extends TypesMap, K> = K extends keyof M
    ? M[K] extends { subtypes: infer S extends NodeBasicInfo[] }
      ? ResolveType<M, S[number]["type"]>
      : K
    : K;

  /**
   * All named nodes' kinds that are usable in ast-grep rule
   * NOTE: SgNode can return kind not in this list
   */
  export type NamedKinds<M extends TypesMap> = ResolveType<M, keyof M>;

  /**
   * See open-ended unions / string literal completion in TypeScript
   * https://github.com/microsoft/TypeScript/issues/26277
   * https://github.com/microsoft/TypeScript/issues/33471
   */
  type LowPriorityKey = string & {};

  /**
   * A union of all named node kinds and a low priority key
   * tree-sitter Kinds also include unnamed nodes which is not usable in rule
   * NOTE: SgNode can return a string type if it is not a named node
   */
  export type Kinds<M extends TypesMap = TypesMap> = NamedKinds<M> | LowPriorityKey;

  /**
   * The root node kind of the tree.
   */
  export type RootKind<M extends TypesMap> = NoNever<
    Extract<M[keyof M], { root: true }>["type"],
    Kinds<M>
  >;

  /** Something to parse along with its path, such as a workflow `forEachFile` file. */
  export interface ParseSource {
    readonly text: string;
    /** Becomes the root's `filename()`; relative paths are against the target. */
    readonly path?: string;
    readonly language?: string;
  }

  export interface ParseOptions {
    language?: string;
  }

  /** Parse `src` as `lang`. */
  export function parse<M extends TypesMap = TypesMap>(lang: string, src: string): SgRoot<M>;
  /**
   * Parse a file, such as the one a workflow `forEachFile` callback receives.
   * The language comes from `options`, then the file, then the step's
   * selector, then the file extension.
   */
  export function parse<M extends TypesMap = TypesMap>(
    file: ParseSource,
    options?: ParseOptions,
  ): SgRoot<M>;
  /**
   * Parse text. Without `options.language`, uses the language of the
   * `forEachFile` step it runs in, when the step's selector names one.
   */
  export function parse<M extends TypesMap = TypesMap>(
    text: string,
    options?: ParseOptions,
  ): SgRoot<M>;
  export function parseAsync<M extends TypesMap = TypesMap>(
    lang: string,
    src: string,
  ): Promise<SgRoot<M>>;
  export function parseAsync<M extends TypesMap = TypesMap>(
    source: ParseSource | string,
    options?: ParseOptions,
  ): Promise<SgRoot<M>>;

  export type TransformOptions<T extends TypesMap> = {
    params: Record<string, string>;
    matches?: SgNode<T>[];
    language: string;
    matrixValues?: Record<string, any>;
    dryRun?: boolean;
    targetDir: string;
  };

  /**
   * @deprecated Use `Codemod` type instead. The name `Transform` is misleading as it implies the function must return a transformed string, but in fact it can also return null for detection purposes.
   */
  export type Transform<T extends TypesMap> = Codemod<T>;

  /**
   * Any JSON a transform reports about its file. Run from a workflow, a
   * `forEachFile` callback returns it, and the step lists it in `outputs`
   * (or folds it with the step's reducer). Explicit `null` is reported;
   * missing or `undefined` data is omitted.
   *
   * @example
   * ```ts
   * return { edit: null, data: { props: props.map((p) => p.text()) } };
   * ```
   */
  export type TransformData =
    | string
    | number
    | boolean
    | null
    | readonly TransformData[]
    | { readonly [key: string]: TransformData };

  /** The new text (`edit`, or `null` for no change), and data to report. */
  export type TransformResult = { edit?: string | null; data?: TransformData };

  export type Codemod<T extends TypesMap> = (
    root: SgRoot<T>,
    options: TransformOptions<T>,
  ) => Promise<string | null | TransformResult>;

  export type GetSelectorOptions<_T extends TypesMap> = {
    params: Record<string, string>;
  };

  export type GetSelector<T extends TypesMap> = ({
    params,
  }: GetSelectorOptions<T>) => RuleConfig<T>;

  /**
   * Execute a transform function on a file, collecting the result.
   *
   * Reads the file at `pathToFile`, parses it with the given `language`,
   * calls the `transformFn` with the parsed root, and returns the
   * transformed content string if the file was modified, or `null` if
   * the file was unchanged.
   *
   * @param transformFn - The transform function to apply
   * @param pathToFile - Path to the file to transform
   * @param language - The language to parse the file as (e.g., "typescript", "javascript")
   * @returns A promise that resolves with the transformed content, or null if unchanged
   *
   * @example
   * ```ts
   * import { jssgTransform } from "codemod/ast-grep";
   *
   * // Apply a transform to a specific file and get the result
   * const result = await jssgTransform(myTransform, "./src/config.ts", "typescript");
   * if (result !== null) {
   *   console.log("Transformed content:", result);
   * }
   * ```
   */
  export function jssgTransform<T extends TypesMap = TypesMap>(
    transformFn: Transform<T>,
    pathToFile: string,
    language: string,
  ): Promise<string | null>;

  /**
   * Create a file (relative to the target) with the step's other changes.
   * Also exported from `codemod/workflow`; see there.
   *
   * @experimental
   */
  export function createFile(path: string, text: string): void;
}

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

    /**
     * A string naming this node across answers and roots: its file, range and
     * kind. Each semantic query wraps nodes in new objects (and other files in
     * new roots), so `===` and `id()` do not match across queries; keys do.
     * Use it in a `Set` or `Map` to dedupe or remember nodes.
     *
     * @example
     * ```ts
     * const seen = new Set<string>();
     * for (const caller of callGraph("typescript").callers.get(fn) ?? []) {
     *   if (seen.has(caller.node.key())) continue; // one edit per function
     *   seen.add(caller.node.key());
     * }
     * ```
     */
    key(): string;

    /**
     * The node in JSON: its key. A node a step returns is its key in the
     * step's result; analyses in later steps take the key where they take a
     * node, and `root.nodeByKey(key)` makes it a node again.
     */
    toJSON(): string;

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
     * const uses = symbols("typescript").references.get(name) ?? [];
     * const own = uses.filter((edge) => fn.contains(edge.node));
     * ```
     */
    contains(other: SgNode<M>): boolean;

    // Mutations (experimental; JavaScript, TypeScript and Python). The call
    // graph, data flow, symbols and types are lookups: `callGraph(lang)`,
    // `dataflow(lang)`, `symbols(lang)`, `types(lang)`. Every answer is a
    // "may" answer: see https://docs.codemod.com/community/jssg/dataflow.

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
     *   and (a TypeError naming the node's kind) when the node is not a
     *   variable or a use of one (for `a.b`, ask about `a`).
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
     * This method is only valid for files reached through `symbols(lang).definition` or
     * `.references` (or another analysis), not the file being processed.
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
     *   const def = symbols("typescript").definition.get(node)?.[0];
     *   const other = def?.node.getRoot();
     *
     *   if (other && other.filename() !== root.filename()) {
     *     // Edit the other file
     *     const edits = [def.node.replace("newVar")];
     *     other.write(other.root().commitEdits(edits));
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
     * What export `name` (`"default"` for the default export) of `module`
     * lands on, following re-exports through barrels and `export *`.
     * `module` is a specifier, resolved from this root's file, or an
     * absolute path (from `symbols(lang).resolveModule()`).
     *
     * @throws when semantic analysis is off for the step, or this root was
     *   parsed from a string.
     * @returns The declaration (`node`, `nameNode`, `root`, `kind`, and
     *   `exportName`: how the declaring module exports it), or null when the
     *   module does not export the name or does not resolve.
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

    /**
     * Whether importing this module runs code: a call made at module top
     * level, an `import "./x"`
     * side-effect import, or a top-level write to an object member
     * (`registry.x = …`). Declarations alone are not effects. A "may" answer:
     * an unresolved call counts even if it is pure, and a side-effect import
     * counts without looking at the imported module.
     *
     * @throws as `resolveExport()` does.
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
     * @throws as `resolveExport()` does.
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
   * The kind of definition that was found.
   * - 'local': Definition is in the same file (local variable, function, class, etc.)
   * - 'import': Definition traced to an import statement but module couldn't be resolved
   * - 'external': Definition resolved to a different file in the workspace
   */
  export type DefinitionKind = "local" | "import" | "external";

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

  /** What `root.resolveExport()` finds: the declaration an export lands on. */
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

  // ---- Call graph ---------------------------------------------------------

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

  // ---- Analyses as values ---------------------------------------------------

  /** A language an analysis covers. */
  export type AnalysisLanguage = "typescript" | "tsx" | "javascript" | "jsx" | "python";

  /** One edge of a lookup: the node it reaches, and what the lookup knows about the hop. */
  export type Edge<To = SgNode, Meta = {}> = { node: To } & Meta;

  /**
   * A node's key, `node.key()`. Across steps, a node is its key: anywhere an
   * analysis takes a node, its key works too.
   */
  export type NodeKey = string;

  /** A lookup row in JSON: `[from, to, meta]` for edges, `[node, edge | null]` for a crawl. */
  export type LookupRow = ReadonlyArray<NodeKey | { readonly [field: string]: unknown } | null>;

  /**
   * A lookup as a step's result, nodes as keys: `lookup` holds
   * `[from, to, meta]` edges and `nodeFields` names the meta fields that
   * held nodes. `lookup(json)` in a later step makes it a lookup again.
   */
  export interface LookupJSON {
    readonly lookup: ReadonlyArray<LookupRow>;
    readonly nodeFields: ReadonlyArray<string>;
  }

  /** A crawl's result as a step's result: `[node, edge | null]` rows. */
  export interface ReachedJSON {
    readonly reached: ReadonlyArray<LookupRow>;
    readonly nodeFields: ReadonlyArray<string>;
  }

  /**
   * A read-only map from a node to the edges that leave it. Nodes match by
   * syntax node (`node.key()`), not by object, so a node from another query,
   * or its key, finds its edges. `get(node)` is `undefined` when no edge
   * leaves it.
   *
   * Built-in lookups (`callGraph`, `dataflow`, `symbols`) are answered node
   * by node and can't be listed: `size`, `keys()` and iteration throw.
   * Lookups from `lookup()` and `join()` list their edges, as a Map does,
   * and can be a step's result (`toJSON()`).
   */
  export interface Lookup<From = SgNode, To = SgNode, Meta = {}>
    extends ReadonlyMap<From, ReadonlyArray<Edge<To, Meta>>> {
    get(node: From | NodeKey): ReadonlyArray<Edge<To, Meta>> | undefined;
    has(node: From | NodeKey): boolean;
    toJSON(): LookupJSON;
    readonly [Symbol.toStringTag]: "Lookup";
  }

  /** Where a call is: the call expression. */
  export interface SiteMeta {
    site: SgNode;
  }

  /** `callGraph(lang)`: who calls a function, and whom it calls (resolved calls only). */
  export function callGraph(language: AnalysisLanguage): {
    callers: Lookup<SgNode, SgNode, SiteMeta & { via: CallEdgeKind }>;
    callees: Lookup<SgNode, SgNode, SiteMeta & { via: CallEdgeKind }>;
  };

  /**
   * One hop of a value: what kind of node, and how it got there.
   * `name` is set for parameters, variables, imports, references, globals
   * and calls (a call's name is the callee text, `"db.query"`); `index` is a
   * parameter's position, or the argument's for `via: "argument"`.
   * `boundary` (sources only): the value comes from outside the analyzed
   * code (a parameter, an import, a global, `this`, a call result), so the
   * trail ends here. "Unknown", not "safe".
   */
  export interface FlowMeta {
    kind: FlowNodeKind;
    via: FlowEdgeKind;
    name: string | null;
    index: number | null;
    boundary: boolean;
  }

  /** `dataflow(lang)`: where a value comes from one hop back, and where it goes one hop on. */
  export function dataflow(language: AnalysisLanguage): {
    sources: Lookup<SgNode, SgNode, FlowMeta>;
    uses: Lookup<SgNode, SgNode, FlowMeta>;
  };

  /**
   * How a file names a module:
   * - 'import': `import … from "m"`, `import "m"`; Python `import m`, `from m import x`
   * - 'export': `export … from "m"`, `export * from "m"`
   * - 'require': `require("m")`
   * - 'dynamic': `import("m")`; Python `importlib.import_module("m")`, `__import__("m")`
   * - 'mock': `jest.mock("m")`, `vi.mock("m")`, ...; Python `mock.patch("m.x")`, `monkeypatch.setattr("m.x", …)`
   */
  export type ModuleRefKind = "import" | "export" | "require" | "dynamic" | "mock";

  /** Where a module is named (`site`: the statement or call), how, and with what specifier. */
  export interface ModuleMeta extends SiteMeta {
    kind: ModuleRefKind;
    specifier: string;
  }

  /**
   * `symbols(lang)`: an identifier's definition and a name's references, in
   * the step's scope (the file, unless the step asks for the workspace), and
   * module resolution (workspace scope).
   *
   * `imports` and `importers` are keyed by a file's root node
   * (`parse(file).root()`) and land on root nodes: the workspace modules a
   * file imports (installed packages left out), and the files that import
   * it, statically, dynamically or in test mocks. Crawl `importers` for
   * every file that depends on a module, directly or not. Workspace scope.
   */
  export function symbols(language: AnalysisLanguage): {
    /**
     * The declaration a name means, through imports (to the import binding
     * when the module does not resolve). One edge: `node` is the
     * declaration (the name, the declarator or the whole `export default`
     * statement), `nameNode` its name, `exportName` how its module exports it.
     *
     * @example
     * ```ts
     * // <Modal /> → export default function Modal() in ./modal.tsx
     * const def = symbols("typescript").definition.get(tag)?.[0];
     * def?.exportName; // "default"
     * def?.nameNode?.text(); // "Modal"
     * def?.node.getRoot().filename(); // ".../modal.tsx"
     * ```
     */
    definition: Lookup<
      SgNode,
      SgNode,
      { nameNode: SgNode | null; kind: DefinitionKind; exportName: string | null }
    >;
    /** Every use of the name, one edge per reference, across files in workspace scope. */
    references: Lookup<SgNode, SgNode>;
    /**
     * The file `specifier` resolves to from `file`, or null. Follows
     * tsconfig/jsconfig `paths` (with `extends`), package `exports`, and
     * index files.
     *
     * @throws when semantic analysis is off for the step, or `file` has no path.
     * @example
     * ```ts
     * symbols("typescript").resolveModule(file, "@/components"); // "/repo/src/components/index.ts"
     * ```
     */
    resolveModule(file: ModuleFile, specifier: string): string | null;
    /**
     * A specifier `file` can import `target` with, checked by resolving it;
     * null when none reaches it. Across workspace packages, only what the
     * target's package makes public (its `exports`, or its entry).
     *
     * @throws as `resolveModule()` does, and on an unknown option.
     * @example
     * ```ts
     * const { specifierFor } = symbols("typescript");
     * specifierFor(file, modal.root, { like: "@/components" }); // "@/components/modal"
     * specifierFor(file, modal.root, { like: "../components" }); // "../components/modal"
     * ```
     */
    specifierFor(file: ModuleFile, target: ModuleFile, options?: SpecifierOptions): string | null;
    imports: Lookup<SgNode, SgNode, ModuleMeta>;
    importers: Lookup<SgNode, SgNode, ModuleMeta>;
  };

  /**
   * A file for `symbols(lang).resolveModule` and `.specifierFor`: an
   * absolute path or one relative to the target, a workflow file
   * (`{ path }`), an SgRoot, or a node in one.
   */
  export type ModuleFile = string | { readonly path: string } | SgRoot | SgNode;

  /**
   * `types(lang)`: an expression's type, or `undefined`: from the
   * TypeScript 7 checker for JavaScript and TypeScript, from ty (Ruff's
   * checker) for Python. Importing `types` turns them on for the step, and
   * `callGraph(lang)` then also resolves method calls by the receiver's type.
   */
  export function types(language: AnalysisLanguage): {
    typeOf: ReadonlyMap<SgNode, string>;
  };

  /** Why a function is an entry point. */
  export interface EntryPoint {
    /** `"export"` for the package's public API, else the declaring rule's `kind`. */
    kind: string;
    /** What reaches it: `"@acme/ui#Button"`, `"GET app/api/users/[id]/route.ts"`. */
    label: string;
  }

  /** `entryPoints(lang)` as a step's result: `[function, reasons]` rows. */
  export interface EntryPointsJSON {
    readonly entryPoints: ReadonlyArray<readonly [NodeKey, ReadonlyArray<EntryPoint>]>;
  }

  /**
   * Every function in the workspace something outside its code calls, mapped
   * to why. Pass it as `crawl()`'s start to walk from all of them.
   */
  export interface EntryPoints extends ReadonlyMap<SgNode, ReadonlyArray<EntryPoint>> {
    get(fn: SgNode | NodeKey): ReadonlyArray<EntryPoint> | undefined;
    has(fn: SgNode | NodeKey): boolean;
    toJSON(): EntryPointsJSON;
  }

  /**
   * `entryPoints(lang)`: every function in the workspace something outside
   * its code calls, and why: its package exports it (`kind: "export"`), or a
   * `semanticAnalysis.entryPoints` rule declares it (the rule's `kind`).
   *
   * @example
   * ```ts
   * const outside = entryPoints("typescript");
   * outside.get(button); // [{ kind: "export", label: "@acme/ui#Button" }]
   * const live = await crawl(callGraph("typescript").callees, outside);
   * ```
   */
  export function entryPoints(language: AnalysisLanguage): EntryPoints;

  /**
   * A lookup from `[from, to, meta?]` edges. Both ends are nodes or their
   * keys (a key resolves against this step's files); the error names the
   * edge that is neither. Built once, read-only.
   *
   * @example
   * ```ts
   * const renders = lookup(pages.map((page) => [page, page.find("<$C $$$/>")!, { via: "jsx" }]));
   * ```
   */
  export function lookup<Meta extends object = {}>(
    edges: Iterable<readonly [SgNode | NodeKey, SgNode | NodeKey] | readonly [SgNode | NodeKey, SgNode | NodeKey, Meta]>,
  ): Lookup<SgNode, SgNode, Meta>;
  /**
   * A lookup an earlier step returned, made a lookup again: its keys name
   * nodes in this step's files, and fail if the file changed since.
   *
   * @example
   * ```ts
   * const routes = await step(() => join({ ... }));
   * await step(async () => crawl([lookup(routes), callees], start));
   * ```
   */
  export function lookup<Meta extends object = any>(json: LookupJSON): Lookup<SgNode, SgNode, Meta>;
  /** A crawl's result an earlier step returned, made one again. */
  export function lookup<Meta extends object = any>(json: ReachedJSON): ReachedMap<Meta>;
  /** `entryPoints(lang)` an earlier step returned, as entry points again. */
  export function lookup(json: EntryPointsJSON): EntryPoints;

  /** One side of a `join`. */
  export interface JoinSide<M extends TypesMap = TypesMap> {
    /** A pattern, or a rule as `findAll()` takes. */
    find: string | { rule: Rule<M> };
    /** The metavariable whose captured text joins the two sides, like "$NAME". */
    on: string;
    /** Which node the edge holds: the match (default), or the function around it. */
    anchor?: "match" | "function";
  }

  /**
   * A lookup from two patterns matched across the workspace: an edge from
   * each `from` match to each `to` match whose `on` metavariables captured
   * the same thing. Built in Rust, in one pass. Each edge has `site` (the
   * `from` match), `name` (the captured text) and `meta`.
   *
   * By default, two captures that both resolve (an imported function, a
   * variable) join when they resolve to the same declaration; when either
   * doesn't (a string, a property without types), when their text is equal.
   * So results get more precise with types on: a tRPC client's
   * `trpc.user.getUser` resolves only through the TypeScript 7 checker.
   * `by: "text"` compares text only; `by: symbols(lang).definition` (or any
   * lookup) resolves only, and drops what doesn't resolve. For text that
   * needs normalizing, build the edges with `lookup()`.
   *
   * @example
   * ```ts
   * // tRPC: a client call reaches the procedure it names.
   * const procedures = join({
   *   from: { find: "trpc.$R.$P.$M($$$)", anchor: "function", on: "$P" },
   *   to: { find: { rule: { pattern: { context: "({ $P: publicProcedure.$M($$$) })", selector: "pair" } } }, on: "$P" },
   *   meta: { via: "trpc" },
   * });
   * ```
   */
  export function join<Meta extends object = {}>(spec: {
    from: JoinSide;
    to: JoinSide;
    meta?: Meta;
    /** How captures are compared; see above. */
    by?: "text" | Lookup<SgNode, SgNode, any>;
    /** The language to match; the step's language when it has one. */
    language?: AnalysisLanguage;
  }): Lookup<SgNode, SgNode, SiteMeta & { name: string } & Meta>;

  /** How `crawl` reached a node: the edge, and the node it left from. */
  export type Reached<Meta = {}> = Edge<SgNode, Meta> & { from: SgNode };

  /** What `crawl` reached: each node, and the edge that reached it (null for a start). */
  export interface ReachedMap<Meta = {}> extends ReadonlyMap<SgNode, Reached<Meta> | null> {
    get(node: SgNode | NodeKey): Reached<Meta> | null | undefined;
    has(node: SgNode | NodeKey): boolean;
    toJSON(): ReachedJSON;
  }

  /**
   * Every node reached from `start` through the lookups' edges, breadth
   * first, each once. Maps each node to the edge that reached it first, and
   * each start node to `null`. Never walks past a node in `stopAt` (it is
   * still reached). Takes lookups only: make a Map into one with `lookup()`,
   * and a lookup an earlier step returned with `lookup(result)`. Start and
   * `stopAt` take nodes or their keys; a lookup, a crawl's result or
   * `entryPoints(lang)` as the start means its nodes.
   */
  export function crawl<Meta = {}>(
    lookups: Lookup<SgNode, SgNode, Meta> | ReadonlyArray<Lookup<SgNode, SgNode, any>>,
    start: SgNode | NodeKey | Iterable<SgNode | NodeKey> | ReadonlyMap<SgNode, unknown>,
    options?: { stopAt?: Iterable<SgNode | NodeKey> },
  ): Promise<ReachedMap<Meta>>;
}

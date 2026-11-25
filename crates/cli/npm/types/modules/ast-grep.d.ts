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

    // Data flow and call graph (experimental; JavaScript and TypeScript only)

    /**
     * Follow this node's value backward: what it is computed from.
     *
     * Steps come nearest first. A step with `boundary: true` is where the
     * analysis stops because the value comes from outside the module's own
     * code: a parameter, an import, a global, `this`, or a call result.
     * The answer is a "may" answer: a variable assigned in two places has
     * both as sources. Flow stays inside the node's file.
     *
     * Returns an empty array when no semantic provider is configured.
     *
     * @experimental
     * @example
     * ```ts
     * // Where does the second argument of query() come from?
     * const arg = call.field("arguments")!.children().filter((n) => n.isNamed())[1];
     * const fromRequest = arg.flowsFrom().some((s) => s.kind === "parameter" && s.name === "req");
     * ```
     */
    flowsFrom(options?: FlowOptions): FlowSteps<M>;

    /**
     * Follow this node's value forward: the variables, calls, returns,
     * property writes and JSX attributes it reaches.
     *
     * A step with `via: "argument"` is a call that receives the value, and
     * `index` is the argument position.
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
     * Functions called from the function at this node (a function, its name,
     * or a call, which answers for the functions it calls).
     *
     * A call whose callee is not a function in the workspace (a library, a
     * builtin, a value the analysis lost track of) comes back with
     * `resolved: false` and the call as its `node`. In file scope, imported
     * functions are unresolved.
     *
     * @experimental
     */
    callees(options?: CallGraphOptions): CallGraphEntries<M>;

    /**
     * Functions that call the function at this node. `node` is the calling
     * function, or the call itself when it is made from top-level code.
     * Needs workspace scope to see callers in other files.
     *
     * In workspace scope the answer comes from a table of every call in the
     * workspace, built on the first `callers()` query of the run. It is a
     * snapshot: files edited after that keep their old calls in it. Answers
     * about the node's own file (`flowsFrom`, `flowsTo`, `mutations`) always
     * use its current content.
     *
     * @experimental
     */
    callers(options?: CallGraphOptions): CallGraphEntries<M>;

    /**
     * Places that change the variable at this node, or the object it holds:
     * reassignments, property writes, mutating method calls, and calls it is
     * passed to.
     *
     * @experimental
     * @example
     * ```ts
     * // Is `items` changed after the audit() call?
     * const changed = items.mutations({ after: auditCall }).some((m) => m.confidence !== "maybe");
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
  }

  export interface DefinitionResult<M extends TypesMap = TypesMap> {
    /** The SgNode at the definition location */
    node: SgNode<M>;
    /** The SgRoot for the file containing the definition */
    root: SgRoot<M>;
    /** The kind of definition (local, import, or external) */
    kind: DefinitionKind;
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

  /** What a step of a data-flow answer is. */
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
   * - 'capture': stored in a new object or array
   * - 'store': written to a property (`o.p = x`)
   * - 'jsx-attribute' / 'jsx-child': the value is passed to a JSX element
   * - 'return': the value is returned from the function
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
     * Treat a call's result as derived from its arguments and receiver.
     * This is the safe assumption for calls the analysis cannot see into.
     * @default true
     */
    throughCalls?: boolean;
    /** Stop after this many steps from the node. */
    maxDepth?: number;
  }

  export interface FlowStep<M extends TypesMap = TypesMap> {
    node: SgNode<M>;
    root: SgRoot<M>;
    kind: FlowNodeKind;
    /** The edge that reached this step. */
    via: FlowEdgeKind;
    /** Variable, property, attribute or callee name, when the step has one. */
    name: string | null;
    /** Parameter position, or argument position when `via` is 'argument'. */
    index: number | null;
    /** Number of edges from the queried node. */
    depth: number;
    /** The value comes from outside the analyzed code; the trail ends here. */
    boundary: boolean;
  }

  /**
   * Steps of a data-flow query. `truncated` is true when a result limit was
   * hit and steps are missing (not set on the Node.js host, which has no
   * semantic provider yet).
   */
  export type FlowSteps<M extends TypesMap = TypesMap> = Array<FlowStep<M>> & {
    truncated?: boolean;
  };

  export interface CallGraphOptions {
    /** Follow calls of calls. @default false */
    transitive?: boolean;
    maxDepth?: number;
  }

  /**
   * Why the call graph has an edge.
   * - 'callback': the function is passed to a call the analysis cannot see
   *   into (`items.map(f)`, `setTimeout(f)`) and is assumed to be called
   * - 'jsx': `<Component />`
   */
  export type CallEdgeKind = "direct" | "method" | "constructor" | "callback" | "jsx";

  export interface CallGraphEntry<M extends TypesMap = TypesMap> {
    /** The function; for an unresolved call, the call. */
    node: SgNode<M>;
    root: SgRoot<M>;
    /** Function name, or the callee's text for an unresolved call. */
    name: string | null;
    /** False when the callee is not a function in the workspace. */
    resolved: boolean;
    via: CallEdgeKind;
    /** Number of calls from the queried function. */
    depth: number;
    /** The call that made this edge. */
    callSite: { node: SgNode<M>; root: SgRoot<M> } | null;
  }

  /** A file the analysis could not parse. */
  export interface UnparsedFile {
    path: string;
    /** The parser's message. */
    message: string;
  }

  /**
   * Entries of a call-graph query.
   * - `truncated`: a result limit was hit and entries are missing
   * - `unparsed`: files the walk needed but could not parse, so calls into
   *   or out of them are missing: imports of the visited functions'
   *   files (`callees`), or workspace files (`callers`)
   *
   * Neither is set on the Node.js host, which has no semantic provider yet.
   */
  export type CallGraphEntries<M extends TypesMap = TypesMap> = Array<CallGraphEntry<M>> & {
    truncated?: boolean;
    unparsed?: UnparsedFile[];
  };

  export interface MutationOptions<M extends TypesMap = TypesMap> {
    /** Only mutations that can run after this node (same file). */
    after?: SgNode<M>;
  }

  export type MutationKind =
    | "reassign"
    | "update"
    | "property-write"
    | "delete"
    | "method-call"
    | "escape";

  export interface Mutation<M extends TypesMap = TypesMap> {
    node: SgNode<M>;
    root: SgRoot<M>;
    kind: MutationKind;
    /**
     * - 'definite': a write in the syntax
     * - 'likely': a method known to mutate its receiver (`push`, `set`, ...)
     * - 'maybe': a call the analysis has no summary for
     */
    confidence: "definite" | "likely" | "maybe";
    /** Property or method name. */
    name: string | null;
    /** In a nested function: runs whenever that function is called. */
    deferred: boolean;
    /** Through another variable that holds the same value. */
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
}

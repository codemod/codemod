/**
 * Build-time split of a workflow module into the trusted workflow (run in
 * Node) and one self-contained JavaScript artifact per inline `jssg()`
 * transform (run in the QuickJS sandbox by the bridge).
 *
 * `buildModule` parses the module with the TypeScript compiler API, finds
 * every `jssg({ ..., transform })` call, bundles each transform with esbuild
 * into an ES module whose default export is the transform (imported helpers
 * inlined, `codemod:*` and sandbox built-ins left as imports), and rewrites
 * the call so `transform` is an `ArtifactRef` (`{ name, hash }`) instead of
 * a function. No function is ever serialized: the transform's source text is
 * taken from the module's own source at the positions the parser reports.
 *
 * Supported subset. A transform may use its own parameters and locals,
 * globals, and bindings the module imports from other modules (those are
 * bundled). It may not use anything else declared in the workflow module
 * (a top-level `const`, `function`, or `class`): dynamic values enter through
 * invocation input, static helpers through imports. Such captures are
 * rejected here with the source position instead of failing in the sandbox.
 * `name` must be a string literal; `transform` must be a method, a function
 * or arrow expression, or an imported binding.
 */
import { createHash } from "node:crypto";
import { readFileSync, realpathSync } from "node:fs";
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { basename, dirname, extname, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { buildSync } from "esbuild";
import ts from "typescript";
import type { ArtifactRef } from "./protocol.ts";

/** One bundled transform. `source` is what the bridge executes; `hash` identifies it. */
export interface JssgArtifact extends ArtifactRef {
  source: string;
  /** Module the transform was extracted from, for messages only. */
  origin: string;
}

export interface BuiltModule {
  /** The module with every inline transform replaced by its `ArtifactRef`. */
  source: string;
  artifacts: JssgArtifact[];
}

/** Artifact lookup by hash; a `Map<string, JssgArtifact>` qualifies. */
export interface ArtifactStore {
  get(hash: string): JssgArtifact | undefined;
}

export class BuildError extends Error {
  constructor(
    readonly file: string,
    readonly position: { line: number; column: number } | undefined,
    message: string,
  ) {
    super(
      position === undefined
        ? `${file}: ${message}`
        : `${file}:${position.line}:${position.column}: ${message}`,
    );
    this.name = "BuildError";
  }
}

const ORCHESTRATION_PACKAGE = "@codemod.com/orchestration";

type Loader = "ts" | "tsx" | "js" | "jsx";

function loaderFor(file: string): Loader {
  switch (extname(file)) {
    case ".tsx":
      return "tsx";
    case ".jsx":
      return "jsx";
    case ".js":
    case ".mjs":
    case ".cjs":
      return "js";
    default:
      return "ts";
  }
}

const SCRIPT_KINDS: Record<Loader, ts.ScriptKind> = {
  ts: ts.ScriptKind.TS,
  tsx: ts.ScriptKind.TSX,
  js: ts.ScriptKind.JS,
  jsx: ts.ScriptKind.JSX,
};

/**
 * Split one module. `file` must be absolute: relative imports of the
 * transforms resolve from its directory. Throws `BuildError` for anything
 * outside the supported subset and for bundling failures.
 */
export function buildModule(source: string, file: string): BuiltModule {
  const loader = loaderFor(file);
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, SCRIPT_KINDS[loader]);
  const fail: (node: ts.Node, message: string) => never = (node, message) => {
    const { line, character } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
    throw new BuildError(file, { line: line + 1, column: character + 1 }, message);
  };
  const module = moduleScope(sf);
  const edits: { start: number; end: number; text: string }[] = [];
  const artifacts: JssgArtifact[] = [];

  for (const call of findJssgCalls(sf)) {
    const argument = call.arguments[0];
    if (argument === undefined || !ts.isObjectLiteralExpression(argument)) {
      fail(call, "jssg() must be called with an object literal so its transform can be extracted");
    }
    const name = literalProperty(argument, "name");
    if (name === undefined) {
      fail(argument, "jssg name must be a string literal");
    }
    const property = argument.properties.find((p) => propertyName(p) === "transform");
    if (property === undefined) {
      fail(argument, `jssg '${name}' has no transform`);
    }
    const entry = transformEntry(property, sf, module, fail, name);
    const bundled = bundle(entry, file, loader);
    const hash = createHash("sha256").update(bundled).digest("hex");
    artifacts.push({ name, hash, source: bundled, origin: file });
    edits.push({
      start: property.getStart(sf),
      end: property.getEnd(),
      text: `transform: ${JSON.stringify({ name, hash })}`,
    });
  }

  let rewritten = source;
  for (const edit of edits.sort((a, b) => b.start - a.start)) {
    rewritten = rewritten.slice(0, edit.start) + edit.text + rewritten.slice(edit.end);
  }
  return { source: rewritten, artifacts };
}

/** Read and split a module on disk. */
export function buildFile(file: string): BuiltModule {
  const absolute = resolve(file);
  return buildModule(readFileSync(absolute, "utf8"), absolute);
}

export interface LoadedWorkflow {
  /** The module namespace; `default` is the workflow or plan. */
  exports: Record<string, unknown>;
  /** Every artifact extracted while the module graph loaded, by hash. */
  artifacts: Map<string, JssgArtifact>;
}

/**
 * Import a workflow module for the trusted-local runner, splitting every
 * module in its graph on the way in: a `node:module` load hook rewrites
 * modules that call `jssg(` (and strips their types) and collects their
 * artifacts. The hook is active only while this import runs. Node caches
 * modules per URL, so artifacts are collected the first time a module loads
 * in a process; `buildFile` is the side-effect-free alternative.
 */
export async function loadWorkflow(file: string): Promise<LoadedWorkflow> {
  const absolute = resolve(file);
  const artifacts = new Map<string, JssgArtifact>();
  // This package's own sources define `jssg`; they never call it.
  const own = resolve(import.meta.dirname) + sep;
  const hooks = registerHooks({
    load(url, context, nextLoad) {
      if (!url.startsWith("file:")) return nextLoad(url, context);
      const path = fileURLToPath(url);
      if (path.startsWith(own) || !/\.(?:[cm]?[jt]s|[jt]sx)$/u.test(path)) {
        return nextLoad(url, context);
      }
      const text = readFileSync(path, "utf8");
      if (!text.includes("jssg(")) return nextLoad(url, context);
      const built = buildModule(text, path);
      for (const artifact of built.artifacts) artifacts.set(artifact.hash, artifact);
      const source = /\.[cm]?ts$/u.test(path)
        ? stripTypeScriptTypes(built.source, { mode: "transform", sourceUrl: url })
        : built.source;
      return { format: "module", source, shortCircuit: true };
    },
  });
  try {
    const exports = (await import(pathToFileURL(absolute).href)) as Record<string, unknown>;
    return { exports, artifacts };
  } finally {
    hooks.deregister();
  }
}

// --- extraction -------------------------------------------------------------

interface ModuleScope {
  /** Import binding name -> its declaration. */
  imports: Map<string, ts.ImportDeclaration>;
  /** Every other top-level value binding -> its declaring node. */
  locals: Map<string, ts.Node>;
}

function moduleScope(sf: ts.SourceFile): ModuleScope {
  const imports = new Map<string, ts.ImportDeclaration>();
  const locals = new Map<string, ts.Node>();
  for (const statement of sf.statements) {
    if (ts.isImportDeclaration(statement)) {
      const clause = statement.importClause;
      if (clause?.name) imports.set(clause.name.text, statement);
      if (clause?.namedBindings) {
        if (ts.isNamespaceImport(clause.namedBindings)) {
          imports.set(clause.namedBindings.name.text, statement);
        } else {
          for (const element of clause.namedBindings.elements) {
            imports.set(element.name.text, statement);
          }
        }
      }
    } else if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        for (const name of bindingNames(declaration.name)) locals.set(name, declaration);
      }
    } else if (
      (ts.isFunctionDeclaration(statement) ||
        ts.isClassDeclaration(statement) ||
        ts.isEnumDeclaration(statement) ||
        ts.isModuleDeclaration(statement)) &&
      statement.name &&
      ts.isIdentifier(statement.name)
    ) {
      locals.set(statement.name.text, statement);
    }
  }
  return { imports, locals };
}

function findJssgCalls(sf: ts.SourceFile): ts.CallExpression[] {
  const calls: ts.CallExpression[] = [];
  const visit = (node: ts.Node) => {
    if (
      ts.isCallExpression(node) &&
      ts.isIdentifier(node.expression) &&
      node.expression.text === "jssg"
    ) {
      calls.push(node);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return calls;
}

function propertyName(property: ts.ObjectLiteralElementLike): string | undefined {
  const name = property.name;
  if (name === undefined) return undefined;
  if (ts.isIdentifier(name) || ts.isStringLiteral(name)) return name.text;
  return undefined;
}

function literalProperty(object: ts.ObjectLiteralExpression, key: string): string | undefined {
  const property = object.properties.find((p) => propertyName(p) === key);
  if (property === undefined || !ts.isPropertyAssignment(property)) return undefined;
  const value = property.initializer;
  return ts.isStringLiteral(value) || ts.isNoSubstitutionTemplateLiteral(value)
    ? value.text
    : undefined;
}

/**
 * The virtual entry module for one transform: the import declarations it
 * needs plus `export default <transform>`.
 */
function transformEntry(
  property: ts.ObjectLiteralElementLike,
  sf: ts.SourceFile,
  module: ModuleScope,
  fail: (node: ts.Node, message: string) => never,
  name: string,
): string {
  const text = (node: ts.Node) => sf.text.slice(node.getStart(sf), node.getEnd());
  let body: string;
  let references: Map<string, ts.Identifier>;
  if (ts.isMethodDeclaration(property)) {
    if (property.asteriskToken) fail(property, `jssg '${name}' transform cannot be a generator`);
    if (property.body === undefined) fail(property, `jssg '${name}' transform has no body`);
    const isAsync = property.modifiers?.some((m) => m.kind === ts.SyntaxKind.AsyncKeyword);
    body = `export default ${isAsync ? "async " : ""}function ${sf.text.slice(property.name.getStart(sf), property.getEnd())}`;
    references = freeIdentifiers(property);
  } else if (ts.isPropertyAssignment(property) || ts.isShorthandPropertyAssignment(property)) {
    const value = ts.isPropertyAssignment(property) ? property.initializer : property.name;
    if (ts.isFunctionExpression(value) || ts.isArrowFunction(value)) {
      body = `export default (${text(value)});`;
      references = freeIdentifiers(value);
    } else if (ts.isIdentifier(value)) {
      body = `export default ${value.text};`;
      references = new Map([[value.text, value]]);
    } else {
      fail(
        value,
        `jssg '${name}' transform must be a method, a function or arrow expression, or an imported binding`,
      );
    }
  } else {
    fail(property, `jssg '${name}' transform must be a method or property`);
  }

  const imports = new Set<ts.ImportDeclaration>();
  for (const [identifier, reference] of references) {
    const declaration = module.imports.get(identifier);
    if (declaration !== undefined) {
      const specifier = (declaration.moduleSpecifier as ts.StringLiteral).text;
      if (
        specifier === ORCHESTRATION_PACKAGE ||
        specifier.startsWith(`${ORCHESTRATION_PACKAGE}/`)
      ) {
        fail(
          reference,
          `jssg '${name}' transform uses '${identifier}' from ${ORCHESTRATION_PACKAGE}; the orchestration runtime is not available inside a transform`,
        );
      }
      imports.add(declaration);
      continue;
    }
    const local = module.locals.get(identifier);
    if (local !== undefined) {
      const { line } = sf.getLineAndCharacterOfPosition(local.getStart(sf));
      fail(
        reference,
        `jssg '${name}' transform uses '${identifier}', declared in the workflow module at line ${line + 1}; a transform can only use its own code, globals, and imported bindings. Move '${identifier}' into a module and import it, or pass it through invocation input`,
      );
    }
  }
  return `${[...imports].map(text).join("\n")}\n${body}\n`;
}

function bindingNames(name: ts.BindingName): string[] {
  if (ts.isIdentifier(name)) return [name.text];
  return name.elements.flatMap((element) =>
    ts.isBindingElement(element) ? bindingNames(element.name) : [],
  );
}

/**
 * Identifiers referenced as values inside `fn` that no scope inside `fn`
 * declares, keyed by name with the first reference. Block, function, `for`,
 * `catch`, class and `switch` scopes are tracked; `var` and function
 * declarations hoist to the enclosing function.
 */
function freeIdentifiers(fn: ts.SignatureDeclaration): Map<string, ts.Identifier> {
  const free = new Map<string, ts.Identifier>();
  const scopes: Set<string>[] = [];
  const declared = (name: string) => scopes.some((scope) => scope.has(name));

  const visit = (node: ts.Node): void => {
    if (ts.isTypeNode(node) || ts.isTypeParameterDeclaration(node)) return;
    if (ts.isInterfaceDeclaration(node) || ts.isTypeAliasDeclaration(node)) return;
    if (ts.isIdentifier(node)) {
      if (isValueReference(node) && !declared(node.text) && !free.has(node.text)) {
        free.set(node.text, node);
      }
      return;
    }
    const scope = scopeOf(node);
    if (scope) scopes.push(scope);
    ts.forEachChild(node, visit);
    if (scope) scopes.pop();
  };
  visit(fn);
  return free;
}

function scopeOf(node: ts.Node): Set<string> | undefined {
  const scope = new Set<string>();
  if (ts.isFunctionLike(node)) {
    if ((ts.isFunctionExpression(node) || ts.isFunctionDeclaration(node)) && node.name) {
      scope.add(node.name.text);
    }
    for (const parameter of node.parameters) {
      for (const name of bindingNames(parameter.name)) scope.add(name);
    }
    if ("body" in node && node.body) hoisted(node.body, scope);
    return scope;
  }
  if (ts.isBlock(node) || ts.isCaseBlock(node)) {
    const statements = ts.isBlock(node)
      ? node.statements
      : node.clauses.flatMap((clause) => [...clause.statements]);
    for (const statement of statements) lexical(statement, scope);
    return scope;
  }
  if (ts.isForStatement(node) || ts.isForInStatement(node) || ts.isForOfStatement(node)) {
    if (node.initializer && ts.isVariableDeclarationList(node.initializer)) {
      for (const declaration of node.initializer.declarations) {
        for (const name of bindingNames(declaration.name)) scope.add(name);
      }
    }
    return scope;
  }
  if (ts.isCatchClause(node)) {
    if (node.variableDeclaration) {
      for (const name of bindingNames(node.variableDeclaration.name)) scope.add(name);
    }
    return scope;
  }
  if (ts.isClassExpression(node) && node.name) {
    scope.add(node.name.text);
    return scope;
  }
  return undefined;
}

/** `let`/`const`/`class`/`function` declared directly by a statement. */
function lexical(statement: ts.Node, scope: Set<string>): void {
  if (ts.isVariableStatement(statement)) {
    for (const declaration of statement.declarationList.declarations) {
      for (const name of bindingNames(declaration.name)) scope.add(name);
    }
  } else if (
    (ts.isFunctionDeclaration(statement) || ts.isClassDeclaration(statement)) &&
    statement.name
  ) {
    scope.add(statement.name.text);
  }
}

/** `var` and function declarations anywhere in a function body, not crossing nested functions. */
function hoisted(body: ts.Node, scope: Set<string>): void {
  const visit = (node: ts.Node): void => {
    if (ts.isFunctionLike(node)) return;
    if (ts.isVariableDeclarationList(node) && (node.flags & ts.NodeFlags.BlockScoped) === 0) {
      for (const declaration of node.declarations) {
        for (const name of bindingNames(declaration.name)) scope.add(name);
      }
    }
    if (ts.isFunctionDeclaration(node) && node.name) scope.add(node.name.text);
    ts.forEachChild(node, visit);
  };
  visit(body);
}

/** False for identifiers that name a property, label, or declaration rather than referencing a binding. */
function isValueReference(identifier: ts.Identifier): boolean {
  const parent = identifier.parent;
  if (ts.isPropertyAccessExpression(parent)) return parent.name !== identifier;
  if (ts.isPropertyAssignment(parent)) return parent.name !== identifier;
  if (ts.isBindingElement(parent)) return parent.propertyName === identifier && false;
  if (
    ts.isMethodDeclaration(parent) ||
    ts.isPropertyDeclaration(parent) ||
    ts.isGetAccessorDeclaration(parent) ||
    ts.isSetAccessorDeclaration(parent) ||
    ts.isPropertySignature(parent) ||
    ts.isEnumMember(parent) ||
    ts.isMetaProperty(parent) ||
    ts.isJsxAttribute(parent)
  ) {
    return parent.name !== identifier;
  }
  if (ts.isLabeledStatement(parent) || ts.isBreakOrContinueStatement(parent)) {
    return parent.label !== identifier;
  }
  if (
    ts.isVariableDeclaration(parent) ||
    ts.isParameter(parent) ||
    ts.isFunctionDeclaration(parent) ||
    ts.isFunctionExpression(parent) ||
    ts.isClassDeclaration(parent) ||
    ts.isClassExpression(parent)
  ) {
    return parent.name !== identifier;
  }
  return true;
}

// --- bundling ---------------------------------------------------------------

/**
 * Bundle a virtual entry into one ES module. Output is deterministic and
 * free of absolute paths (module comments are relative to the module's real
 * directory, which esbuild also uses for every import it resolves), so the
 * hash is stable across checkouts. Node built-ins and `codemod:*` stay
 * external; everything else is inlined.
 */
function bundle(entry: string, file: string, loader: Loader): string {
  const directory = realDirectory(dirname(file));
  try {
    const result = buildSync({
      stdin: { contents: entry, resolveDir: directory, sourcefile: basename(file), loader },
      absWorkingDir: directory,
      bundle: true,
      write: false,
      format: "esm",
      platform: "node",
      target: "es2022",
      external: ["codemod:*"],
      legalComments: "none",
      logLevel: "silent",
      charset: "utf8",
    });
    return result.outputFiles[0]!.text;
  } catch (error) {
    const messages = (error as { errors?: { text: string; location?: { line: number } }[] }).errors;
    const detail = messages?.map((m) => m.text).join("; ") ?? (error as Error).message;
    throw new BuildError(file, undefined, `failed to bundle a jssg transform: ${detail}`);
  }
}

function realDirectory(directory: string): string {
  try {
    return realpathSync.native(directory);
  } catch {
    return directory;
  }
}

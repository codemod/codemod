/**
 * Helpers the inline transforms call. A transform runs in the QuickJS sandbox
 * and may use only its own code, globals, and bindings imported from other
 * modules, so shared logic lives here: the build step bundles this module into
 * every artifact that imports it. Only types are imported, so the bundles have
 * no runtime dependencies (and nothing from `@codemod.com/orchestration`,
 * which does not exist inside a transform).
 */
import type { Edit, SgNode, SgRoot, TypesMap } from "@codemod.com/jssg-types/main";

/** The file's path relative to the target root, with forward slashes. */
export function fileOf<M extends TypesMap>(root: SgRoot<M>): string {
  return root.relativeFilename().replaceAll("\\", "/");
}

/** Every `callee(argument)` call with exactly one argument. */
export function callsTo<M extends TypesMap>(root: SgRoot<M>, callee: string): SgNode<M>[] {
  return root.root().findAll({ rule: { pattern: `${callee}($ARG)` } });
}

export function countCalls<M extends TypesMap>(root: SgRoot<M>, callee: string): number {
  return callsTo(root, callee).length;
}

/** `callee(x)` calls whose argument is not yet an object literal. */
export function unwrappedCalls<M extends TypesMap>(root: SgRoot<M>, callee: string): SgNode<M>[] {
  return callsTo(root, callee).filter((call) => !argumentOf(call).is("object"));
}

export interface Rewrite {
  /** The new file content, or `null` when nothing changed. */
  content: string | null;
  /** How many call sites changed. */
  replaced: number;
}

/** `from(x)` becomes `to(x)`. */
export function rewriteCalls<M extends TypesMap>(
  root: SgRoot<M>,
  from: string,
  to: string,
): Rewrite {
  return commit(
    root,
    callsTo(root, from).map((call) => call.replace(`${to}(${argumentOf(call).text()})`)),
  );
}

/** `callee(x)` becomes `callee({ name: x })` where `x` is not already an object. */
export function wrapCalls<M extends TypesMap>(root: SgRoot<M>, callee: string): Rewrite {
  return commit(
    root,
    unwrappedCalls(root, callee).map((call) =>
      call.replace(`${callee}({ name: ${argumentOf(call).text()} })`),
    ),
  );
}

function argumentOf<M extends TypesMap>(call: SgNode<M>): SgNode<M> {
  const argument = call.getMatch("ARG");
  if (argument === null) throw new Error(`${call.text()} has no argument`);
  return argument;
}

function commit<M extends TypesMap>(root: SgRoot<M>, edits: Edit[]): Rewrite {
  return {
    content: edits.length === 0 ? null : root.root().commitEdits(edits),
    replaced: edits.length,
  };
}

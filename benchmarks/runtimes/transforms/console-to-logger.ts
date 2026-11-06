/**
 * Benchmark transform 1: `console.log(...)` -> `logger.info(...)`.
 *
 * Only the callee is replaced, so arguments (including nested
 * `console.log(...)` calls, which get their own non-overlapping edit) keep
 * their exact text. Returns `null` for files without a match so both engines
 * record the file as unmodified.
 *
 * Shared verbatim by the YAML workflow (`js_file`) and the TypeScript workflow
 * (imported binding, bundled into an artifact). Type-only imports, no
 * selector, no semantic analysis, no module-level state.
 */
import type { SgRoot } from "codemod:ast-grep";
import type TS from "codemod:ast-grep/langs/typescript";

export default function consoleToLogger(root: SgRoot<TS>): string | null {
  const program = root.root();
  const calls = program.findAll({ rule: { pattern: "console.log($$$ARGS)" } });
  const edits = [];
  for (const call of calls) {
    const callee = call.field("function");
    if (callee !== null) edits.push(callee.replace("logger.info"));
  }
  if (edits.length === 0) return null;
  return program.commitEdits(edits);
}

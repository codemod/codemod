# Fallback JSSG Guidance

This file is a fallback used only when the public JSSG docs cannot be fetched at runtime.

The public docs are the source of truth for:
- JSSG quickstart
- runtime and built-ins
- API reference
- advanced patterns
- testing
- semantic analysis
- import utilities

When the public docs are available, prefer them over this file.

## Core rules

- Use JSSG for Codemod JS/TS codemods.
- Prefer TypeScript.
- Return `null` when a file should not change.
- Apply collected edits with `root.root().commitEdits(edits)`.
- Use workflow `include`/`exclude` globs instead of filtering files inside the transform when possible.

## Runtime and capabilities

- JSSG is a QuickJS runtime with LLRT-based Node compatibility.
- Standard Node-style imports are available in JSSG; some modules are capability-gated.
- If the codemod uses gated APIs such as `fs`, `fetch`, or `child_process`, update `codemod.yaml` in the same change with the matching `capabilities` entry.
- For related multi-file JSSG work, prefer `jssgTransform` or other JSSG APIs before falling back to shell steps.

## Patterns are AST shapes, not text

- ast-grep patterns are parsed as syntax, not matched as raw text.
- Meta-variables must match whole AST nodes.
- Do not write partial meta-variable patterns like `foo$BAR`; capture the whole node and constrain it instead.

## Relational rules and `stopBy`

- `has`, `inside`, `precedes`, and `follows` can use `stopBy` to control search depth.
- `stopBy: "neighbor"` limits the search to immediate neighbors.
- `stopBy: "end"` searches until the structural boundary.
- Use `stopBy` when you need “any ancestor/descendant until a boundary” rather than only direct containment.

## Minimal transform shape

```ts
export default async function transform(root) {
  const program = root.root();
  const edits = [];

  // collect edits

  return edits.length > 0 ? program.commitEdits(edits) : null;
}
```

## Semantic analysis reminder

Configure semantic analysis under a `js-ast-grep` step inside `nodes[].steps[]`, for example:

```yaml
nodes:
  - id: transform
    name: Transform code
    steps:
      - name: Run codemod
        js-ast-grep:
          js_file: scripts/codemod.ts
          semantic_analysis: workspace
```

## MCP tool usage

- Use `dump_ast` to inspect AST shapes before finalizing patterns.
- Use `get_node_types` when you need the tree-sitter node type map for a language.
- Use `run_jssg_tests` when you want MCP-assisted test execution.
- Use `get_jssg_utils_instructions` for import helper usage.
- If available in the current MCP build, use the runtime/capabilities guidance resource before introducing shell steps or capability-gated APIs.

## If behavior is uncertain

Prefer the public JSSG docs and the current CLI help over guessing.

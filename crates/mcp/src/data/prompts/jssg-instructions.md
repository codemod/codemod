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

- Use JSSG for Codemod transformation scripts. For JS/TS-family source edits, prefer `js-ast-grep` packages.
- Prefer TypeScript.
- Return `null` when a file should not change.
- Apply collected edits with `root.root().commitEdits(edits)`.
- Use workflow `include`/`exclude` globs instead of filtering files inside the transform when possible.
- Before reaching for regex or manual parsing, use the verified KB tools and `dump_ast` to understand the AST shape you are actually matching.
- For source transforms, do not use `RegExp`, `.replace`, `.replaceAll`, `.match`, `.split`, or manual string parsing as the primary implementation strategy.
- Minimal string operations are acceptable only for path normalization, import/module-specifier cleanup, helper metadata formatting, or test-output parsing.

## Runtime and capabilities

- JSSG is a QuickJS runtime with LLRT-based Node compatibility.
- Standard Node-style imports are available in JSSG; some modules are capability-gated.
- Prefer normal Node-style imports in codemods. Do not invent shell wrappers just to reach APIs that JSSG already exposes.
- If the codemod uses gated APIs such as `fs`, `fetch`, or `child_process`, update `codemod.yaml` in the same change with the matching `capabilities` entry.
- For related multi-file JSSG work, prefer `jssgTransform` or other JSSG APIs before falling back to shell steps.
- For detailed runtime and capability rules, read `jssg-runtime-capabilities-instructions` from Codemod MCP.

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
- Use `get_jssg_runtime_capabilities` before introducing capability-gated APIs or shell steps for related multi-file work.
- Use `get_jssg_utils_instructions` for import helper usage.

## If behavior is uncertain

Prefer the public JSSG docs and the current CLI help over guessing.

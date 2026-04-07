# Supplemental Codemod Creation Guidance

This file is a local supplement for agent workflow policy.

Public Codemod docs remain the source of truth for:
- CLI syntax
- package/workflow semantics
- JSSG syntax
- fixture layout

Use this file only for the extra agent guidance that public docs should not carry.

## Default authoring loop

1. Search the registry before deciding to create a new package.
2. Inspect 1-3 representative repo files after or alongside registry discovery.
3. If there is no exact package, scaffold immediately.
4. Replace starter transform/README/fixtures immediately after scaffold.
5. Define positive, negative, and edge fixtures before deep implementation.
6. Implement the deterministic transform.
7. Run tests, validate the workflow, and validate the package before stopping.

## Hot-path caveats

- If Codemod MCP is missing from the callable tool list, stop and tell the user to fix MCP visibility first.
- Call `get_jssg_gotchas` and `get_ast_grep_gotchas` before writing source-transform code.
- Use `dump_ast` when the pattern shape is unclear.
- If symbol origin matters, use semantic analysis and binding-aware checks.
- After a registry miss, use `scaffold_codemod_package` immediately.
- Use `validate_codemod_package` before stopping.
- Do not create commits or push branches unless the user explicitly asked for git operations.

# Fallback Sharding Guidance

This file is a fallback used only when the public workflow and sharding docs cannot be fetched at runtime.

The public docs are the source of truth for:
- shard step usage
- matrix-from-state patterns
- Campaign/cloud PR behavior
- built-in shard methods
- custom shard functions

When the public docs are available, prefer them over this file.

## Minimal reminders

- `shard` is a step action that writes shard results to workflow state.
- A common shape is:
  1. evaluate shards into state
  2. run a matrix node from that state
- In Campaign/cloud runs, shard tasks get their own branches and can produce their own pull requests.
- `pull_request.title` customizes shard PR metadata when the task ends with commits.
- Shard names are useful inputs for `branch_name`.

## Minimal structure

```yaml
nodes:
  - id: evaluate-shards
    steps:
      - name: Build shards
        shard:
          method:
            type: directory
            max_files_per_shard: 20
          output_state: shards

  - id: apply-transforms
    strategy:
      type: matrix
      from_state: shards
    steps:
      - name: Run transform
        js-ast-grep:
          js_file: scripts/codemod.ts
```

## If exact semantics are uncertain

Use the public workflow reference and sharding docs instead of relying on this fallback.

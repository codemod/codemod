# Sharding

The `shard` step action splits large migrations into multiple PRs by evaluating which files a codemod would modify and grouping them into shards. Each shard becomes a matrix task that creates its own PR.

## How it works

Sharding workflows follow a two-node pattern:

1. **evaluate-shards** — Runs the `shard` step to discover applicable files, group them, and write shard assignments to workflow state.
2. **apply-transforms** (matrix) — Iterates over the shards with a matrix strategy, applying the codemod and creating one PR per shard.

```yaml
version: "1"

state:
  schema:
    shards:
      type: array
      items:
        type: object

nodes:
  - id: evaluate-shards
    name: Evaluate shards
    steps:
      - name: Build shards
        shard:
          method:
            type: directory
            max_files_per_shard: 20
          target: "./src"
          output_state: shards
          js-ast-grep:
            js_file: scripts/codemod.ts
            language: tsx
            include: ["**/*.{ts,tsx}"]

  - id: apply-transforms
    name: Apply transforms
    trigger:
      type: manual
    depends_on: [evaluate-shards]
    strategy:
      type: matrix
      from_state: shards
    pull_request:
      title: "refactor: migrate ${{ matrix.name }}"
    steps:
      - name: Run transform
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: tsx
```

## File discovery

The engine pre-filters files before passing them to any shard method. This is shared across both built-in and custom methods.

When `js-ast-grep` is set on the shard step, the engine:
1. Globs files matching `include` under `target`
2. Dry-runs the codemod against each file
3. Keeps only files where the transform produces changes

### Shard step parameters

- **`shard.target`** (string, default: `"."`): Root directory to scan for files. Defaults to the workflow run target (project root).
- **`shard.output_state`** (string, required): State key to write shard results to. Must match the state schema key referenced by `from_state` in the matrix node.
- **`shard.file_pattern`** (string): Glob pattern for eligible files. Used when `js-ast-grep` is not set.
- **`shard.js-ast-grep`** (object): JSSG codemod configuration for pre-filtering. When set, the engine dry-runs the codemod and only shards files where the transform produces changes.

## Built-in methods

Built-in methods handle grouping and bin-packing automatically. Set the `method.type` to choose an algorithm.

### Directory

Groups files by their immediate subdirectory under `target`, then bin-packs into shards.

```yaml
shard:
  method:
    type: directory
    max_files_per_shard: 20
    min_shard_size: 5
  target: "./src"
  output_state: shards
  js-ast-grep:
    js_file: scripts/codemod.ts
    language: tsx
    include: ["**/*.{ts,tsx}"]
```

Each shard includes:
- `name` — `"{directory}-{index}"` (e.g. `"components-0"`)
- `directory` — the subdirectory path
- `_meta_files` — files in the shard

### Codeowner

Groups files by their owning team from `.github/CODEOWNERS` (or root `CODEOWNERS`), then bin-packs into shards.

```yaml
shard:
  method:
    type: codeowner
    max_files_per_shard: 30
  target: "./src"
  output_state: shards
  file_pattern: "**/*.tsx"
```

Each shard includes:
- `name` — `"{team}-{index}"` (e.g. `"platform-team-0"`)
- `team` — the owning team
- `_meta_files` — files in the shard

### Method parameters

- **`method.type`** (string, required): `directory` or `codeowner`.
- **`method.max_files_per_shard`** (number, required): Target number of files per shard.
- **`method.min_shard_size`** (number): Minimum shard size. Trailing shards smaller than this are merged into the previous shard.

## Custom shard functions

For grouping logic that built-in methods can't express (e.g. dependency-aware clustering), point `method.function` to a JS/TS file that runs inside the jssg engine.

```yaml
shard:
  method:
    function: scripts/shard-by-deps.ts
  target: "./src"
  output_state: shards
  js-ast-grep:
    js_file: scripts/codemod.ts
    language: tsx
    include: ["**/*.{ts,tsx}"]
```

The function receives the pre-filtered file list and returns shard groupings. The engine handles all file I/O — the function only handles grouping.

### Function signature

```typescript
import type { ShardInput, ShardResult } from "codemod:workflow";

export default function shard(input: ShardInput): ShardResult[] {
  const { files, targetDir, previousShards } = input;
  // Custom grouping logic
  return [
    { name: "shard-0", _meta_shard: 0, _meta_files: [...] },
  ];
}
```

### ShardInput

| Field | Type | Description |
|-------|------|-------------|
| `files` | `string[]` | Relative paths of eligible files (pre-filtered by the engine) |
| `targetDir` | `string` | Absolute path to the target directory |
| `previousShards` | `ShardResult[]` | Previous shard assignments for incremental re-evaluation |

### ShardResult

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Shard identifier (used in PR titles, branch names) |
| `_meta_shard` | `number` | Shard index |
| `_meta_files` | `string[]` | Files in this shard |
| Any other key | `unknown` | Exposed as `${{ matrix.<key> }}` variables |

> Fields prefixed with `_meta_` are excluded from the matrix hash. This means re-indexing shards won't invalidate existing task identity.

### Available APIs

Custom shard functions run in the jssg engine with full access to:

- **`codemod:ast-grep`** — Parse files, match patterns, navigate ASTs. Useful for building dependency graphs from import statements.
- **`codemod:workflow`** — Types (`ShardInput`, `ShardResult`) and state management APIs.

```typescript
import { parse } from "codemod:ast-grep";
import type { ShardInput, ShardResult } from "codemod:workflow";

export default function shard(input: ShardInput): ShardResult[] {
  const { files, targetDir } = input;

  // Use ast-grep to parse imports and build a dependency graph
  for (const file of files) {
    const root = parse("tsx", targetDir + "/" + file);
    const imports = root.root().findAll({
      rule: { kind: "string_fragment", inside: { kind: "import_statement" } },
    });
    // ... build graph from imports
  }

  // Group files by connected components, bin-pack into shards
  return groupAndPack(files, graph);
}
```

## Re-evaluation

When the target repo changes (files added, moved, or deleted), retry the evaluate-shards task. The engine re-evaluates shards with **incremental stability**:

- **Existing assignments are preserved** — a file already assigned to shard 1 stays in shard 1.
- **New files go to new shards** — they never get added to shards whose tasks are already completed or in progress.
- **Empty shards are dropped** — if all files in a shard were deleted, its task is marked `WontDo`.

This means re-running the shard step never disrupts work that's already in flight.

For custom shard functions, the engine passes `previousShards` in the input so the function can implement its own incremental logic.

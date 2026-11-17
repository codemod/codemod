# Side by side: YAML engine vs TS orchestration

Setup:
- **YAML runs** used `codemod 1.13.18`, built in release mode from the branch (`/home/user/ts-orch/target/release/codemod`).
- **That binary validates the same way `main` does:**
  - `crates/cli/src/commands/workflow/validate.rs` is identical on the branch and on `main`.
  - So is `core/src/utils.rs::validate_workflow`.
  - So are `models/src/step.rs` and `node.rs`, and `main` has no `deny_unknown_fields` either.
- **TS checks** used `tsc --noEmit` with the package's own tsconfig (`@codemod.com/tsconfig/lib.json`, strict), run on copies in `/tmp`.
- **TS workflows were not run.** The napi addon is not built in this environment.

## (a1) Same workflow, two engines: the benchmark (2 JSSG steps)

| | YAML `benchmarks/runtimes/yaml/workflow.yaml` | TS `benchmarks/runtimes/ts/workflow.ts` |
|---|---|---|
| Lines (non-comment) | 22 | 19 |
| Transform files | the same 2 files, referenced by path (`js_file`, which must be inside the workflow dir: a symlink is needed) | the same 2 files, imported (`import consoleToLogger from "../transforms/..."`) |
| Diff on vite v8.3.6 | identical (sha256 `8f7ff40f…c1c95885`, `HANDOFF.md:237`) | identical |

```yaml
nodes:
  - id: benchmark
    name: Runtime benchmark transforms
    type: automatic
    steps:
      - name: console-to-logger
        js-ast-grep:
          js_file: transforms/console-to-logger.ts
          language: typescript
          include: ["**/*.{ts,mts,cts,js,mjs,cjs}"]
          exclude: ["**/node_modules/**"]
      - name: rename-vitest-import
        js-ast-grep: { js_file: transforms/rename-vitest-import.ts, ... same globs ... }
```

```ts
const files = { language: "typescript", include: ["**/*.{ts,mts,cts,js,mjs,cjs}"], exclude: ["**/node_modules/**"] } as const;
const consoleStep = jssg({ name: "console-to-logger", ...files, transform: consoleToLogger });
const importStep  = jssg({ name: "rename-vitest-import", ...files, transform: renameVitestImport });
export default sequence(consoleStep(), importStep());
```

**Verdict:** for a straight line of transforms the two are equally small. TS saves duplication by spreading `files` into each step and by type-checking the transform import. The difference shows once data flows between steps.

## (a2) Data flow, a conditional and a parallel group: `demo/06-dynamic.ts`

What the workflow does:
1. Inspect the project with a shell step.
2. Migrate only if something is pending.
3. Run a verification and a usage analysis in parallel.
4. Return a typed result.

**TS (`packages/orchestration/demo/06-dynamic.ts`, 9 code lines plus shared step definitions):**

```ts
export default dynamic(async () => {
  const inventory = await inspect();                                   // shell, stdout validated as Inventory
  const migrated = inventory.pending > 0 ? await renameCalls() : [];   // jssg, Migration[] aggregated per file
  const [{ remaining }, usage] = await parallel(verifyNoLegacy(), migratedCalls());
  return { pending: inventory.pending, migrated, remaining, usage };
});
```

The step definitions are shared by every demo: `lib/steps.ts` (54 lines), `lib/ast.ts` (46), `lib/schemas.ts` (56) and `04-parallel.ts` (35). About 40 of those lines are used here.

**YAML equivalent, written for this comparison.** It validated and **ran successfully** on a copy of `demo/fixture`: 38 workflow lines + 2 transforms (17 + 16 lines).

```yaml
nodes:
  - id: inspect
    name: Inspect
    steps:
      - name: inspect
        run: |
          n=$(grep -r --include='*.ts' --exclude='*.d.ts' --exclude='*.generated.ts' -l 'oldApi(' src | wc -l | tr -d ' ')
          echo "pending=$n" >> "$STATE_OUTPUTS"          # data out: key=value lines in a file
  - id: rename-calls
    name: Rename calls
    depends_on: [inspect]
    steps:
      - name: rename-calls
        if: state.pending > 0                            # data in: an untyped expression string
        js-ast-grep: { js_file: transforms/rename-calls.ts, language: typescript, include: ["src/**/*.ts"], exclude: [...] }
  # the "parallel group" is two nodes that depend on the same node
  - id: verify-no-legacy
    depends_on: [rename-calls]
    steps: [{ name: verify-no-legacy, run: '... echo "remaining=$n" >> "$STATE_OUTPUTS"' }]
  - id: migrated-calls
    depends_on: [rename-calls]
    steps: [{ name: migrated-calls, js-ast-grep: { js_file: transforms/migrated-calls.ts, ... } }]
```

```ts
// transforms/rename-calls.ts: per-file results are aggregated by hand into shared state, under a lock
import { acquireLock, getState, setState } from "codemod:workflow";
export default function transform(root) {
  const calls = root.root().findAll({ rule: { pattern: "oldApi($ARG)" } });
  if (calls.length === 0) return null;
  const edits = calls.map((c) => c.replace(`newApi(${c.getMatch("ARG")!.text()})`));
  const release = acquireLock("migrated");
  try {
    const migrated = getState("migrated") ?? [];
    migrated.push({ file: root.filename(), replaced: edits.length });
    setState("migrated", migrated);
  } finally { release(); }
  return root.root().commitEdits(edits);
}
```

Result of the YAML run: `Workflow completed in 0.1s`. The run's "output" is whatever ended up in the shared state file, `~/.local/share/butterflow/state/<run-id>.json`:
`{"pending":2,"migrated":[{file:"/abs/path/src/utils.ts",replaced:1},…],"remaining":0,"usage":[…]}`.

| Concern | YAML | TS |
|---|---|---|
| Passing data between steps | 3 mechanisms here: `$STATE_OUTPUTS` file, the `state.*` namespace, `codemod:workflow` `setState` + `acquireLock` | return values |
| Types on that data | none (`key=value`, JSON-or-string, `engine.rs:4268-4285`) | schema-validated at run time, inferred by `tsc` |
| Conditional | `if:` string, evaluated with evalexpr at run time | a TS `if` / ternary |
| Parallel group | DAG shape: sibling nodes with the same `depends_on` | `parallel(a(), b())` returns a typed tuple |
| Aggregating per-file results | the author takes a lock and does read-modify-write on shared state | the engine aggregates per-file `output` into an array in file order |
| Workflow result | a side effect in the state JSON | the typed return value (and in history) |
| `steps.<id>.outputs` | not wired into conditions or steps (`engine.rs:2508`, `:3143`, `:4038` TODO) | n/a |

## (b) Author errors: what each engine reports

Each case was run for real. The outputs below are trimmed copies of the actual tool output.

### TS (`tsc --noEmit`, ~3.6 s for the demo set including the package sources)

| # | Mistake | Result |
|---|---|---|
| T1 | Typo in a step field: `inclde:` in `jssg({...})` | **caught**: `07-input.ts(17,3): error TS2561: Object literal may only specify known properties, but 'inclde' does not exist in type 'JssgOptions<"typescript", Config, Migration[]>'. Did you mean to write 'include'?` |
| T2 | Wrong param type: `migrateTo({ input: { replacement: 42 } })` | **caught**: `error TS2769: No overload matches this call. … Type 'number' is not assignable to type 'string'.` |
| T3 | Missing transform: `import consoleToLogger from "./transforms/console-to-logger.ts"` (the file does not exist) | **caught**: `error TS2307: Cannot find module './transforms/console-to-logger.ts' or its corresponding type declarations.` |
| T4 | Typo reading a step's output: `inventory.pendng > 0` | **caught**: `06-dynamic.ts(18,30): error TS2551: Property 'pendng' does not exist on type 'Inventory'. Did you mean 'pending'?` |
| T5 | Stage mismatch: `sequence(inspect(), verifyMigrations())` (Inventory → needs Migration[]) | **caught, but the message is opaque**: `error TS2345: Argument of type '[Command<Inventory, void>, Command<Verification, Migration[]>]' is not assignable to parameter of type 'never'.` |
| T6 | Stage mismatch into a **parallel group**: the `05-composed.ts` plan with the verify group moved before `renameCalls()`, and `sequence(inspect(), parallel(verifyNoLegacy(), verifyMigrations()))` | **not caught by `tsc`** (exit 0). The header comment of `05-composed.ts` says this reorder "is a type error", which is wrong. The input schema should reject it at run time; that was not run. |

### YAML (`codemod workflow validate -w <file>`)

| # | Mistake | Result |
|---|---|---|
| Y1 | Typo in a step field: `inclde:` under `js-ast-grep` | **silently accepted** (`✅ Workflow definition is valid`). Unknown keys are ignored (no `deny_unknown_fields`). **Verified at run time:** with `inclde:` on the 06 port's rename step, the run exits 0 and also rewrites `scripts/tool.ts` outside `src/`, because the step falls back to every file of the language. |
| Y2 | Wrong param type: `max_threads: four` | **caught at parse** with a good message: `YAML error: nodes[0].steps[0]: invalid type: string "four", expected usize at line 16 column 9 … help: Check nodes[0].steps[0] … expected usize, but got string "four".` |
| Y3 | Missing transform: `js_file: transforms/console-to-loger.ts` | **caught**: `Workflow validation error: JS file referenced in workflow not found: y3/transforms/console-to-loger.ts` |
| Y4 | Typo in the step kind: `js-ast-grp:` | **caught, vague**: `no variant of enum StepAction found in flattened data at line 16 column 9` |
| Y5 | Bad dependency: `depends_on: [benchmrk]` | **caught**: `Node benchmark depends on non-existent node: benchmrk` |
| Y6 | Bad enum value: `type: automatik` | **caught**: ``unknown variant `automatik`, expected `automatic` or `manual` `` |
| Y7 | Undefined param in a condition: `if: params.replacment == "newApi"` | **passes validate.** Expressions are not checked. |
| Y8 | Typo reading state (the T4 equivalent): `if: state.pendng > 0` in the 06 port | **passes validate. The run exits 0 (`Workflow completed in 0.1s`) and the migration is silently skipped:** `src/app.ts` still has 2 `oldApi(` calls. |

**Verdict.**
- **Structural mistakes:** YAML catches most of them (enum values, field types, missing `js_file`, dangling `depends_on`).
- **Data-flow mistakes:** YAML misses all of them: unknown keys, `params.*`/`state.*` references, step outputs. Y8 is a silent wrong result.
- **TS:** catches all of these at compile time except a mismatch into a `parallel(...)` group (T6), a gap in `composition.ts` typing worth fixing before claiming "typed end to end".

## (c) Adding a new step type

**YAML.** Evidence: the shard step was added in commit `2db5083e`, which touched 16 files (+2,434 lines).

| # | File | Why | Compiler-enforced? |
|---|---|---|---|
| 1 | `crates/models/src/step.rs` | `StepAction` variant + `Use<X>` struct (serde, schemars and ts-rs derives) | n/a |
| 2 | `crates/core/src/step_executor.rs:61-269` | match arm in the step dispatch | yes (exhaustive `match`) |
| 3 | `crates/core/src/engine.rs` (or a new module + `lib.rs`) | `execute_<x>_step`, `${{ }}` resolution of its fields | n/a |
| 4 | `crates/core/src/utils.rs:132-333` | `validate_workflow`: path/glob checks for the new fields | no (`if let` chain) |
| 5 | `crates/cli/src/utils/package_validation.rs:457`, `:607` | package behaviour summary | no (`_` arm) |
| 6 | `schemas/workflow.json` | regenerate via `xtask` (`xtask/src/main.rs:40-46`) | no |
| 7 | `packages/jssg-types` | only if transforms see a new API | no |
| 8 | docs (`docs/workflows/reference.mdx`) + MCP prompts | | no |
| 9 | codemod-app: `packages/validators/src/workflow-butterflow.ts` (zod generated from the schema) | the platform UI and validators | no |

Minimum: **6 files in 3 crates + the generated schema, plus the platform's generated validator.**

**TS.**

| # | File | Why | Compiler-enforced? |
|---|---|---|---|
| 1 | `src/core/protocol.ts:22-198`, `:511-550` | `<X>Operation` + add it to the `Operation` union + `isOperation` case | partly |
| 2 | `src/authoring/runnable.ts` | `x(options)` factory: encode/decode, options type | n/a |
| 3 | `src/execution/<x>.ts` (new) | executor | n/a |
| 4 | `src/execution/scheduler.ts:130-143` | `weightOf` case | **yes** (switch without default) |
| 5 | `src/host/runtimes/node-executor.ts:74-101` | dispatch case | no (has `default`) |
| 6 | `src/host/quickjs/entry.ts:87-105` (+ Rust host functions in `crates/orchestration-quickjs-host` if it needs new native I/O) | dispatch case | no |
| 7 | `src/index.ts`, `src/portable.ts` | exports | no |
| 8 | `fixtures/protocol/*.json` | wire fixtures (if the op crosses to Rust) | no |

Minimum: **7 TS files (1 new), 0 Rust, no schema generation.** Two runtimes are kept in sync by hand, which is the cost of having quickjs as well as node.

**The bigger difference: most YAML "new step types" are not engine changes in TS.**
- Templates, matrices, conditionals and custom shard functions are plain TS functions over `shell`/`jssg`/`dynamic`, and live in the workflow package.
- The engine changes only for a new *kind of side effect*, such as a new executor.
- In YAML, every reusable behaviour beyond `templates` (string inputs) or nested `codemod` packages needs an engine change.

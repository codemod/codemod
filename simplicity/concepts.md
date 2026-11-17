# Concepts an author or contributor has to learn

Sources:
- **YAML:** `codemod/codemod` `main` @ `0d66d565`. Paths are under `crates/` unless noted.
- **TS:** `prototype/typescript-orchestration` @ `72865ebd`. Paths are under `packages/orchestration/` unless noted.

Each item is cited as file:line.

## Summary

| | YAML | TS |
|---|---:|---:|
| Author-facing building blocks | 8 step kinds + node (manual/automatic, `depends_on`, trigger, matrix, runtime) + templates + params/state schemas | 7 functions (`shell`, `jssg`, `agent`, `assessment`, `sequence`, `parallel`, `dynamic`) + schemas |
| Model types an author can write (`schemas/workflow.json` definitions / TS option interfaces) | **35** definitions, **111** properties | **4** runnable option interfaces (25 fields in total) + 3 composition functions + invocation `{id, input, target}` |
| Status/state enums an author or operator sees | WorkflowStatus 6, TaskStatus 7 (+ NodeType 2, TriggerType 2) | CompletionStatus 4 |
| Data-passing mechanisms between steps | 5 (see below) | 1: return values, typed by input/output schemas |
| Embedded languages | YAML + `${{ }}` expressions (evalexpr) + shell + JS/TS transforms | TS only (plus shell strings) |
| Where wiring errors are caught | partly when the YAML is parsed, the rest at run time or never (see `side-by-side.md`) | `tsc` (mostly) + schema validation at run time |
| Engine-internal enums a contributor meets | WorkflowEvent 12, WorkflowCommand 11, StepPhase, DiffOperation 4, TaskErrorDetails 2 | RunEvent 9, HistoryEvent 3, NondeterminismKind 5 |

## YAML engine: author-facing model (`crates/models`, `schemas/workflow.json`)

| Concept | Where | Variants / fields |
|---|---|---|
| Workflow | `models/src/workflow.rs:17` | `version`, `state`, `params`, `templates`, `nodes` |
| Workflow `state` schema / `params` schema | `workflow.rs:43`, `:51` → `SimpleSchema` `models/src/schema.rs:10` | SimpleSchemaType **5** (`string`, `number`, `array`, `object`, `boolean`) `schema.rs:35`; string `oneOf`/`enum`, `multi_line`, `secret` |
| Node | `models/src/node.rs:23` | 12 fields: `id`, `name`, `description`, `type`, `depends_on`, `trigger`, `strategy`, `runtime`, `steps`, `env`, `branch_name`, `pull_request` |
| NodeType | `node.rs:13` | **2**: `automatic`, `manual` |
| Trigger / TriggerType | `models/src/trigger.rs:18` / `:8` | **2**: `automatic`, `manual` (overlaps with NodeType) |
| Strategy (matrix) | `models/src/strategy.rs:15`, StrategyType `:8` | **1** type (`matrix`); `values` or `from_state` |
| Runtime / RuntimeType | `models/src/runtime.rs:30` / `:7` | **3**: `direct`, `docker`, `podman`; 6 fields |
| Step | `models/src/step.rs:7` | `id`, `name`, action (flattened), `env`, `if`, `commit` |
| StepAction (step kinds) | `step.rs:42` | **8**: `use` (template), `run`, `ast-grep`, `js-ast-grep`, `codemod`, `ai`, `install-skill`, `shard` |
| Step kind payloads | `step.rs:78` TemplateUse (2), `:90` UseAstGrep (6), `:122` UseJSAstGrep (9), `:204` UseCodemod (4), `:226` UseAI (13), `:293` UseInstallSkill (5), `:341` UseShard (5) | 44 fields |
| SemanticAnalysisConfig / Mode | `step.rs:174` / `:184` | 2 forms × 2 modes (`file`, `workspace`) |
| Shard method / builtin type | `step.rs:375` / `:405` | 2 (`builtin`, custom `function`) / 2 (`directory`, `codeowner`) |
| InstallSkillHarness / Scope | `step.rs:320` / `:332` | 7 / 2 |
| CommitConfig, PullRequestConfig | `step.rs:424`, `:489` | 3 + 4 fields |
| Template (reusable step group) | `models/src/template.rs:49`; inputs `:11`, outputs `:35` | `id`, `name`, `runtime`, `inputs`, `steps`, `outputs`, `env` |
| `${{ }}` expressions | `models/src/variable.rs:210`; conditions `variable.rs:177` (evalexpr) | namespaces `params.*`, `state.*`, `matrix.*`, `steps.<id>.outputs.*`, `task.id`, `task.signature`, `task.<CODEMOD_TASK_* env>` (`variable.rs:104-170`); array expansion in lists (`variable.rs:297`) |

## YAML engine: run-time concepts an author has to know

| Concept | Where | Notes |
|---|---|---|
| WorkflowStatus | `workflow.rs:105` | **6**: Pending, Running, Completed, Failed, AwaitingTrigger, Canceled |
| Task (runtime instance of a node; matrix master + children) | `models/src/task.rs:48` | `is_master`, `master_task_id`, `matrix_values` |
| TaskStatus | `task.rs:9` | **7**: Pending, Running, Completed, Failed, AwaitingTrigger, Blocked, WontDo |
| Shared state + diffs | `models/src/state_diff.rs:48`, DiffOperation `:8` | **4**: Add, Update, Remove, Append |
| Data passing 1: shell `STATE_OUTPUTS` file | `core/src/engine.rs:4224`, parsed `:4268-4272` | lines `key=value` or `key@=value` (append); the value is JSON or else a string |
| Data passing 2: `CODEMOD_STATE` input file | `engine.rs:4208` | path to a JSON file holding the current state |
| Data passing 3: `codemod:workflow` in transforms | `codemod-sandbox/src/workflow_global/mod.rs:460-467` | `setStepOutput`, `getStepOutput`, `getOrSetStepOutput`, `setState`, `getState`, `unsetState`, `acquireLock` (a lock is needed when parallel per-file transforms aggregate) |
| Data passing 4: `steps.<id>.outputs` | `variable.rs:135-137` | **not loaded for `if:` conditions or step execution**: `// TODO: Load step outputs from STEP_OUTPUTS file` at `engine.rs:2508`, `:3143`, `:4038` |
| Data passing 5: template inputs/outputs | `template.rs:11`, `:35` | string-typed inputs |
| Other env contract | `engine.rs:4218`, `:4233`, `:4236` | `CODEMOD_PATH`, `CODEMOD_TASK_ID`, `CODEMOD_WORKFLOW_RUN_ID` |
| Built-in transform modules | `codemod-sandbox/src/sandbox/engine/execution_engine.rs:423-442` | `codemod:ast-grep`, `codemod:workflow`, `codemod:metrics`, `codemod:llm`, `codemod:runtime` |
| Capabilities | `engine.rs:654-740`; per-step `UseJSAstGrep.capabilities` `step.rs:122` | `--allow-fs`, `--allow-fetch`, `--allow-child-process` + an interactive prompt |
| CLI surface for `workflow run` | `cli/src/commands/workflow/run.rs:30-90` | 15 options + 2 global (`--param`, `--allow-dirty`, `--dry-run`, 3× `--allow-*`, `--no-interactive`, `--agent`, `--install-skill`, `--report`, `--format`, …); plus `resume`, `status`, `list`, `cancel`, `tui`, `validate` subcommands |

Contributor-only types:
- Event and command bus: WorkflowEvent **12** variants (`core/src/workflow_runtime.rs:40`) and WorkflowCommand **11** (`:105`).
- StepPhase (`engine.rs:230`).
- TaskErrorDetails **2** (`task.rs:34`).
- StateAdapter local/cloud/mock (`state/src/*`).

## TS engine: author-facing primitives

| Primitive | Where | Options |
|---|---|---|
| `shell(...)` | `src/authoring/runnable.ts:108`; options `:84` | `name`, `command` (string or `(input) => string`), `env`, `input`, `output` (stdout parsed as JSON and validated) |
| `jssg(...)` | `runnable.ts:197`; options `:159` | `name`, `language`, `include`, `exclude`, `semanticAnalysis`, `selector` (static prefilter), `input`, `output`, `transform` (inline or imported) |
| `agent(...)` | `runnable.ts:344`; options `:294`, backends `:289` | `name`, `prompt`, `backend` (**3**: `builtin` {tools, maxSteps} / `claude-code` {tools} / `codex` {sandbox}), `input`, `output` |
| `assessment(...)` | `runnable.ts:511`; options `:424` | `name`, `include`, `exclude`, `input`, `ask(file, input)`, `model` |
| `sequence(...)` | `src/authoring/composition.ts:136` | static stages; returns the last output |
| `parallel(...)` | `composition.ts:149` | static group; returns a tuple in declaration order |
| `dynamic(fn)` | `src/authoring/dynamic.ts:9` | arbitrary async TS; `await step()` inside |
| Invocation | `src/authoring/command.ts:32-44` | `step()`, `step({ id })`, `step({ input })`, `jssg` only: `step({ target })`; Target `src/core/protocol.ts:36` (`root`, `include`, `exclude`) |
| Schemas | `src/authoring/schema.ts:6` (Standard Schema), `guard()` `:46` | any Standard Schema library (zod, valibot, …) |
| Transform result | `src/authoring/transform.ts:78` | `string | null | undefined | { content, output }` |

## TS engine: run-time concepts

| Concept | Where | Variants |
|---|---|---|
| CompletionStatus | `src/core/protocol.ts:20` | **4**: succeeded, failed, cancelled, unknown |
| Operation kinds | `protocol.ts:198` | **4**: shell, jssg, agent, assessment |
| History (append-only) | `src/core/history.ts:19` | HistoryEvent **3**: scheduled, completed, finalized |
| Replay | `src/runtime/gate.ts:26` | NondeterminismKind **5** (`src/core/errors.ts:15`): changed, reordered, added, removed, output |
| Run events | `src/core/events.ts:5` | **9**: `command.scheduled/replayed/completed`, `run.finished`, `scheduler.queued/admitted/released/paused/resumed` |
| Runtimes | `src/runtime/capabilities.ts:34` | **2**: `node` (default), `quickjs` |
| Error classes | `src/core/errors.ts:4-79`, `src/authoring/context.ts:52`, `capabilities.ts:65`, `schema.ts:21` | OperationError, NondeterminismError, DuplicateCommandIdError, InvocationError, TargetValidationError, CompositionValidationError, NoActiveRunError, UnsupportedInRuntimeError, SchemaError |
| CLI surface | `src/host/cli.ts:87` | `run <wf.ts> [--runtime] [--target] [--input <json>] [--dashboard]`, `build --runtime quickjs -o` |

## Rules a TS author has to learn

These rules are the TS engine's own complexity, so they are listed here too.

| Rule | Where |
|---|---|
| Repeating an invocation of the same runnable needs an explicit `id`. Without one, it throws DuplicateCommandIdError. | `README.md:185` |
| Static stages consume the flowing input when they have an input schema and ignore it otherwise; only `step({ input })` binds input. | `README.md:193` |
| An inline transform may not capture top-level declarations of the workflow module. Values go in through `input`, helpers through imports. | `src/bundle/build.ts:15-21` |
| Module-level state in a transform persists across files and commands, and is unsupported. | `RUNTIMES.md:427` |
| `dynamic` bodies must be deterministic for replay. Otherwise the run fails with NondeterminismError. | `src/runtime/gate.ts:26`, `errors.ts:15` |
| Node runtime: transforms cannot import `codemod:workflow`, `codemod:metrics`, `codemod:llm` or `codemod:runtime`. Data leaves through the return value. | `RUNTIMES.md:418` |

## Reading this

- **What the TS engine removes:** the 5 YAML data channels (state file, state JSON, `codemod:workflow` + locks, step outputs that are only partly wired, template inputs) become **one**: return values typed by schemas. Two status enums with 13 states become one completion status with 4. `if:` strings, matrix and templates become plain TS (`if`, `.map` with `parallel`, functions).
- **What the TS engine adds:** replay/determinism, invocation ids, transform-capture limits and input-flow rules. These are fewer than the YAML concepts, but subtler, and the errors for them come at run time, not from the type checker.
- **What has no TS equivalent yet,** so no concept is needed for it: manual nodes/triggers, shards, nested codemods, install-skill, commit/PR config, container runtimes, capabilities. See `parity-gaps.md`.

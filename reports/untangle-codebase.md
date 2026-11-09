# Codemod untangle-codebase report

Analysis-only pass of [untangle-codebase](https://github.com/caldoai/skills/blob/main/skills/untangle-codebase/SKILL.md) against `codemod/codemod`. No production code changed.

| | |
|---|---|
| Generated | 2026-11-08 · America/Los_Angeles |
| Base | `main @ 0d66d565` |
| Interactive twin | [`untangle-codebase.html`](./untangle-codebase.html) |

**36** problems · **10** fixes · **0** code changes

## How to read a problem

| Category | Meaning |
|---|---|
| **hidden-link** | Pieces that must change together, but the code doesn’t show it |
| **needless-link** | Pieces tied together that change for different reasons |
| **surplus** | No reason of its own to change (wrapper, dead, speculative) |
| **special-case** | Exists for one situation instead of a design that accounts for it |

**Pays off** = what fixing it buys: agents read/change less, fewer silent bugs, less bloat.

**Safety** = risk of applying the fix. **Impact** = agent leverage of the parent fix.

## Priority matrix

| | Safety low | Safety medium | Safety high |
|---|---|---|---|
| **Impact high** | **Start:** F2, F5 | **Next:** F1, F3, F4, F7 | **Plan:** F6 |
| **Impact med** | F9, F10 | F8 | — |

## All problems

### F1 · Terminal ownership (impact high · safety medium)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `non-cli-stdout` | hidden-link | Core, runners, state, and sandbox print to the terminal even though only CLI should own user-facing output. | Fewer silent TUI/JSONL corruptions; agents stop reintroducing prints outside CLI. |
| `quiet-call-order` | hidden-link | Quiet/TUI ownership is a ritual across `run.rs`, `workflow/run.rs`, `tui/mod.rs`, `workflow_runner`, and `create_engine`. | One typed mode; agents change presentation once instead of missing a site and leaking stdout. |
| `dual-run-config` | hidden-link | `create_engine` returns a cloned `WorkflowRunConfig`; quiet hits the engine copy while some fields need dual-writes. | Fewer “I set the flag but nothing changed” bugs on run paths. |
| `jsonl-println` | special-case | `--format jsonl` reaches the engine, but CLI still `println!`s summaries onto the same stream. | CI parsers can trust format flags. |
| `filediff-print` | needless-link | `FileDiff::print` lives in core, so every dry-run path must know quiet/color/JSONL. | Core stays data-only; presentation changes don’t force engine edits. |
| `direct-runner-quiet` | needless-link | `DirectRunner` prints based on a quiet bit instead of always returning captured lines. | Runners stop coupling to CLI TUI conventions. |
| `session-attach-quiet` | hidden-link | `WorkflowSession::attach` mutates quiet + injects callbacks after construction. | Session vs CLI becomes an explicit mode, not call-order folklore. |

**Fix:** one `RunPresentationMode`; non-CLI crates return events/lines; CLI alone renders.

### F2 · Capability & language catalog (impact high · safety low)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `cap-sync` | hidden-link | Adding a capability means hand-editing llrt-capabilities, engines, publish externals, and jssg-types with no compiler link. | One catalog entry instead of half-wired modules. |
| `mod-dup` | needless-link | `codemod:*` registration is copy-pasted across five engine entry points. | Cuts boilerplate; new modules can’t be forgotten in one path. |
| `native-wasm-ast` | hidden-link | Native uses `codemod:ast-grep`, WASM uses `ast-grep`; types claim native-only APIs on both. | Fewer “works locally, fails WASM” import bugs. |
| `langdetect` | hidden-link | The same `.jsx` path maps to jsx, javascript, or tsx depending on factory vs sandbox vs CLI. | Predictable provider/parser choice. |
| `dynlang` | special-case | Dynamic language inventories are hand-maintained in loader, WASM, and jssg-types. | Less inventory drift; adding a language is one generated change. |
| `walker-dup` | needless-link | `WorkspaceWalker` enums exist in factory and language-javascript and must be remapped by hand. | Removes duplicate types with no benefit. |

**Fix:** generate one capability/language catalog for Rust + TS; single module-install helper.

### F3 · Typed matrix / cancel / preview (impact high · safety medium)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `matrix-meta` | hidden-link | Shard/scheduler/engine share a `_meta_` string convention with no shared type. | Renames/filters become type-checked; shard identity harder to break silently. |
| `recompile-hash` | hidden-link | State-change detection and task identity use incompatible hashers; nothing says their inputs must stay different. | Shared hasher + explicit policies; avoids “unify the hashes” bugs that recreate shards or miss recompiles. |
| `cancel-string` | special-case | Cancel is `Failed` + the exact string `"Canceled by user"`. | Cancel is first-class; agents can’t typo a magic string. |
| `pro-dry-run` | special-case | One preview caller is four independent engine flags in the schedule loop. | Preview lives in one profile; no hunting every `if` flag. |

**Fix:** typed `MatrixMeta`, `TaskStatus::Canceled`, `ExecutionProfile`. Share a hasher **implementation**; keep separate state-change vs task-identity **inputs** (still exclude `_meta_` from identity).

### F4 · EventBus + one log writer (impact high · safety medium)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `global-events` | hidden-link | Events go through a process-global map keyed by run id; publishers assume someone else registered a sink. | Nested runs/tests stop losing or cross-talking events. |
| `parallel-log-writers` | hidden-link | Shell and JSSG idle paths mutate `task.logs` / publish outside `TaskStateService::append_task_log`. | Log + event stay twin; fewer “saved but UI didn’t wake” bugs. |

**Fix:** inject `EventBus`; only `TaskStateService` writes task logs/events.

### F5 · Shared post-run presenter (impact high · safety low)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `three-run-paths` | hidden-link | `run`, `workflow run`, and `jssg run` each copy metrics → report → serve/print. | Report/metrics change once; agents don’t update two of three paths. |
| `output-rs-misnamed` | surplus | `commands/output.rs` only formats harness install errors; real output is scattered `println!`s. | Cuts a naming trap; agents find the real emit path. |

**Fix:** one post-run presenter; real CLI emit module.

### F6 · Split Engine (impact high · safety high)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `engine-god` | needless-link | `engine.rs` (~5.2k LOC) owns scheduling, task lifecycle, and every step kind. | Agents change one concern without loading the whole runtime. |
| `dual-exec-paths` | hidden-link | Main loop runs tasks sync “for matrix recompilation”; resume/trigger spawn threads. | One executor + explicit barrier; fewer race/recompile bugs. |
| `step-executor-thin` | surplus | `StepExecutor` only forwards back into Engine — a hop without moving ownership. | Removes pass-through bloat when finding real step logic. |
| `swallowed-errors` | special-case | Recompile `Err` is logged and continued; condition errors become “skip” via `unwrap_or_default`. | Failures surface as failures; “why didn’t it run?” has a real error. |

**Fix:** split into scheduler loop / step runtime / task lifecycle. Do after F1/F3/F4.

### F7 · Registry auth unification (impact high · safety medium)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `registry-auth-split` | hidden-link | CLI and core have parallel auth types; publish reimplements multipart HTTP outside `RegistryClient`. | Auth/registry contract changes once; download and publish stay aligned. |

### F8 · Split harness / report piles (impact medium · safety medium)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `harness-god` | surplus | `harness_adapter.rs` (~6.8k LOC) mixes harnesses, MCP, updates, locks, skill install. | Change one harness without loading unrelated install/update code. |
| `harness-lock-sleep` | special-case | Lock contention is a sleep/retry loop with local constants, not an explicit protocol. | Clearer concurrency; fewer “tweak the sleep” race patches. |
| `report-server-pile` | surplus | `report_server.rs` piles HTTP, OIDC, feedback spawn, and idle timeout together. | Report UX changes don’t risk auth/feedback regressions. |

### F9 · Move expression eval out of models (impact medium · safety low)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `expr-in-models` | needless-link | Models crate owns ~1808 LOC of expression evaluation bound to `CODEMOD_TASK_*` env. | Schema stays separate from runtime eval; models isn’t a second engine. |

### F10 · Surplus cleanup (impact medium · safety low)

| ID | Category | Problem | Pays off |
|---|---|---|---|
| `typeinfo-dead` | surplus | `type_info` / `get_type` stubs exist but nothing implements them and types don’t export them. | Deletes speculative API agents keep “supporting” for no value. |
| `noop-unused` | surplus | `NoopSemanticProvider` is unused outside its unit tests; factory returns `None` instead. | Removes a fake second routing story. |
| `shallow-exports` | needless-link | `node-exports.ts` / `web-exports.ts` are one-line re-exports. | Less hop bloat on every API rename. |
| `gen-artifacts` | surplus | Committed generated TS has the wrong generator name in its header. | Clear SoT; fewer wasted turns editing generated files. |
| `factory-bypass` | needless-link | Semantic fixtures construct Oxc/Ruff providers directly; production uses `LazySemanticProvider`. | Factory routing regressions get caught by fixtures. |
| `init-lang-matrix` | hidden-link | Adding a js-astgrep language needs templates + `init.rs` consts/match/fixtures; unknown langs silently become JS. | Data-driven table + hard error; no wrong-language fallback. |

## Recommended order

1. **F1 + F5** — CLI presentation ownership  
2. **F2** — generated catalog + CI inventory check  
3. **F3 + F4** — typed conventions + explicit events  
4. **F6** — Engine split (highest risk; after the above)  
5. **F7–F10** — opportunistic when touching those areas  

Comments/Tests skill passes not run.

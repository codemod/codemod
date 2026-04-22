# JSSG Runtime and Capabilities

Use this guide when a codemod needs Node-style runtime APIs, capability-gated modules, or non-trivial multi-file JSSG behavior.

## Runtime model

- JSSG runs on QuickJS with LLRT-based Node compatibility.
- Standard Node-style imports such as `fs`, `path`, `process`, and `child_process` are available through the JSSG runtime surface when enabled.
- Prefer normal Node-style imports in codemods. Do not invent shell wrappers just to reach APIs that JSSG already exposes.

## Safe vs capability-gated modules

Safe modules are available by default and do not require `codemod.yaml` capabilities. Common examples:

- `assert`
- `buffer`
- `console`
- `crypto`
- `events`
- `fs` (sandboxed — see below)
- `os`
- `path`
- `perf_hooks`
- `process`
- `stream/web`
- `string_decoder`
- `timers`
- `tty`
- `url`
- `util`
- `zlib`

### `fs` is sandboxed by default

`fs` and `fs/promises` are available without any capability entry, but are constrained to the workflow's target directory:

- Reads, writes, `readdir`, `mkdir`, `stat`, `exists`, `unlink` all work for paths inside `target_dir`.
- Paths that normalize outside `target_dir` throw with `err.code === "EACCES"`.
- Missing paths inside `target_dir` throw with `err.code === "ENOENT"`.
- `.` and `..` path segments are normalized before the prefix check. On real-disk runs the resolver also rejects any path that traverses a symlink (checked via `symlink_metadata`), so symlinks inside the repo cannot be used to escape the sandbox.

Use the default sandboxed fs whenever the codemod only touches files inside the repo. Do not add `fs` to `capabilities` just because the codemod imports it.

### Capability-gated (require explicit opt-in)

- `fs` (unrestricted) — upgrades the sandboxed fs to LLRT's real-disk `fs`, removing the `target_dir` prefix check. Only add this if the codemod genuinely reads or writes paths outside the target directory (home config, caches, `/tmp`, etc.).
- `fetch` — network requests. Fully deny-by-default.
- `child_process` — process spawning. Fully deny-by-default.

If the codemod imports or relies on one of these gated APIs, update `codemod.yaml` in the same change. Do not leave the transform using gated APIs without the matching capability declaration.

```yaml
capabilities:
  - fs             # only when paths outside target_dir are needed
  - fetch
  - child_process
```

Only declare the capabilities the codemod actually needs.

## Package update rule

When you add or remove a gated runtime dependency:

1. Update `codemod.yaml` in the same change.
2. Keep the capability list minimal.
3. Briefly document the capability usage in the package README or usage notes only if the codemod actually needs the capability.

Example:

- If the codemod reads adjacent config files with `fs` that live inside the target directory, **no capability entry is needed** — the default sandboxed fs covers it.
- If it reads files outside the target directory (e.g. `~/.config/...`), add `fs`.
- If it calls an HTTP API with `fetch`, add `fetch`.
- If it shells out to another tool with `child_process`, add `child_process`.

## Prefer JSSG over shell for related multi-file work

If the migration touches multiple related files but remains AST-safe, keep the work inside JSSG.

- Use `jssgTransform(...)` to transform secondary files that are part of the same migration hop.
- Use `root.rename(...)` when the codemod needs to rename the file it is already transforming.
- Do not introduce a shell step just to locate or mutate a second related file when JSSG can handle the hop directly.

Example:

- If a component import changes from `./styles.less` to `./styles.css`, keep the import rewrite and the adjacent stylesheet transform in JSSG instead of adding a second custom shell command that needs to know the stylesheet path.

Shell/native steps remain acceptable only when:

- the user explicitly asked for shell/native implementation, or
- there is no viable ast-grep/JSSG path for the required change.

## Type guidance

- `@codemod.com/jssg-types` includes the JSSG AST types and LLRT-backed module declarations used by the runtime.
- Import runtime modules using their normal names, for example:
  - `import { readFileSync } from "fs"`
  - `import path from "path"`
  - `import process from "process"`
- If a runtime API is documented as available in JSSG, prefer using it directly instead of routing through shell commands.

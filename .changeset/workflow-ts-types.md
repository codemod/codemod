---
"@codemod.com/jssg-types": minor
---

The type declarations now ship with the `codemod` package, the CLI itself (`npm install codemod`, then `"types": ["codemod"]`, or import from `codemod/main` and `codemod/langs/*`), so they are versioned with the CLI. This package mirrors them, so existing imports keep working.

New in the declarations: types for authoring `workflow.ts`: `step` (including async function steps), `forEachFile` (async callbacks, optional selector, `concurrency`, `metrics` in the result), the tasks (`jssg`, `shell`, `shard`, `astGrep`, `codemod`, `agent`, `installSkill`, `approval`, `branch`, `commit`, `pullRequest`), `getState()`, and `WorkflowContext`. Step results are typed and read-only, and values that are not JSON (a `Set`, a `Map`, a `Date`, a function) are type errors where a step result or callback return value must be JSON.

`parse` in `codemod/ast-grep` also accepts a file (`parse(file)`, `parse(file, { language })`) and text with options (`parse(text, { language })`), next to the existing `parse(lang, text)`. `MetricAtom` is now declared correctly inside the `codemod/metrics` module.

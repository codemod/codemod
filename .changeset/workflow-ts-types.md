---
"@codemod.com/jssg-types": minor
---

Add types for authoring `workflow.ts`: `step`, `forEachFile`, `parse`, the tasks (`jssg`, `shell`, `shard`, `astGrep`, `codemod`, `agent`, `installSkill`, `approval`, `branch`, `commit`, `pullRequest`), `getState()`, and `WorkflowContext`. Step results are typed and read-only, and values that are not JSON (a `Set`, a `Map`, a `Date`, a function, a `Promise`) are type errors where a step result or callback return value must be JSON.

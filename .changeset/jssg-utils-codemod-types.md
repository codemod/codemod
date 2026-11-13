---
"@jssg/utils": minor
---

Import types from the `codemod` package (`codemod/main`, `codemod/langs/*`) instead of `@codemod.com/jssg-types`. `codemod` is an optional peer dependency: projects that use these utils already depend on the `codemod` CLI, which brings the types.

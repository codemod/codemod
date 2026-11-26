---
"@jssg/utils": minor
---

The binding helpers (`importBindingOf`, `resolveDefinition`, `resolvesToFactory`, `renameReferences`) answer through `symbols(lang).definition` and `.references`, since the `definition()` and `references()` node methods are gone. On a CLI that still has the node methods they use those. `resolveDefinition` now follows an import through re-exports to the declaration, not one hop.

# codemod

## 0.18.11

### Patch Changes

- Updated dependencies [0c466c7]
  - @codemod.com/workflow@0.0.33

## 0.18.10

### Patch Changes

- c74c9f9: Remove update alert print

## 0.18.9

### Patch Changes

- 865a8b2: Fix a bug in relative path detection
- Updated dependencies [865a8b2]
  - @codemod.com/workflow@0.0.32

## 0.18.8

### Patch Changes

- b1ff177: Release to the legacy channel

## 0.18.7

### Patch Changes

- d6b318d: Release codemod CLI to the legacy channel

## 0.18.6

### Patch Changes

- 6734200: Refactored deprecated endpoints to return a 410 GONE error, disabled all CLI commands except codemod run

## 0.18.5

### Patch Changes

- 0467787: Add content type for publish request header

## 0.18.4

### Patch Changes

- 3d5d78e: Make ast-grep runner unicode-aware

## 0.18.3

### Patch Changes

- d1da807: fix: Fix telemetry

## 0.18.2

### Patch Changes

- baea1b7: fix: Improve node modules resolution for node codemod symlinking step

## 0.18.1

### Patch Changes

- 4b59d50: fix: add child process support for specific codemod execution

## 0.18.0

### Minor Changes

- fd6ccca: Delete auto bump version in publish step

## 0.17.0

### Minor Changes

- d0c2110: Change the README.md boilerplate to use the new format for the codemod init command.

### Patch Changes

- @codemod.com/workflow@0.0.31

## 0.16.0

### Minor Changes

- d8c7c67: Fixed issues with the Codemod CLI, including resolving the build failure on the publish command, removing automatic dependency installation during the init command, fixing .gitignore in package-boilerplate.ts, and removing the LICENSE file from the boilerplate list.

## 0.15.0

### Minor Changes

- b0dae1b: Add API keys functionality

### Patch Changes

- 006e9f8: Fixed fallback in case keytar cannot be used (fixes issue with authorization in headless environment)

## 0.14.2

### Patch Changes

- bf840a4: Clearer instructions for running a recipe with some codemods using the CLI.

## 0.14.1

### Patch Changes

- b677d74: Support exported parser variable from the codemod file, include esm/cjs -specific file extensions to included patterns

## 0.14.0

### Minor Changes

- daea47d: add support for ESM

### Patch Changes

- 6497280: pass engine while executing run
  - @codemod.com/workflow@0.0.31

## 0.13.11

### Patch Changes

- c78ebc5: handles execution error when file path could not be read from
- 29cbaa3: stdout noop bugfix

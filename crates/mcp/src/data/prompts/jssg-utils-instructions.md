# Fallback JSSG Import Utilities Guidance

This file is a fallback used only when the public JSSG utils docs cannot be fetched at runtime.

The public docs are the source of truth for import utility details. When they are available, prefer them over this file.

## Minimal reminders

- The import helpers live under `@jssg/utils/javascript/imports`.
- The main helpers are:
  - `getImport`
  - `addImport`
  - `removeImport`
- These helpers work on the program/root node and return lookup info or edits.
- Apply returned edits with `root.root().commitEdits(...)`.

## If exact behavior is uncertain

Prefer the public JSSG utils docs over this fallback.

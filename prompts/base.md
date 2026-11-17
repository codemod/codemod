Migrate this repository, codeimage (a pnpm monorepo), from Solid 1.9 to Solid 2. Target solid-js 2.0.0 (2.0.0-rc.23 is also fine: its types are identical). The official migration guide (from the solid-js 2.0.0 source) is at DOCS_DIR/MIGRATION.md.

Scope is core Solid only. These libraries have no Solid 2 release yet. Keep them as they are: don't rewrite, vendor, fork, stub or replace them, and don't change how the app uses them. Type errors that come only from using them are expected and out of scope:
@kobalte/core, @kobalte/utils, @kobalte/vanilla-extract, @motionone/solid, @thisbeyond/solid-dnd, solid-codemirror, solid-headless, solid-mdx, solid-toast, @solid-aria/types, @solid-aria/utils, solid-motionone.
Other Solid ecosystem packages (for example @solidjs/router, @solid-primitives/*, solid-use, the TanStack Solid packages and the Vite plugin) have Solid 2 releases on npm; move them to those.

You are done when:
- every package that uses Solid declares solid-js 2 (with @solidjs/web where the code needs it), including the pnpm catalog;
- project source no longer uses Solid 1 APIs that Solid 2 removed or renamed;
- `pnpm install` succeeds, and `pnpm libs:build` succeeds;
- `pnpm exec tsc --noEmit -p .` in each of apps/codeimage, apps/website, packages/ui, packages/atomic-state, packages/dom-export, packages/highlight, packages/locale and packages/config shows no errors, except errors caused by the libraries listed above and these errors that already exist on Solid 1:
  - apps/codeimage: 6 implicit-any errors in src/mocks/projectMockHandlers.ts, src/pages/Dashboard/components/ProjectItem/ProjectItem.tsx and src/state/editor/editor.ts
  - apps/website: 9 errors in CodeEditor.tsx (solid-codemirror deep import), src/core/object.ts, src/ui.ts and scripts/vercel-output-build.ts
Ignore apps/api.

Don't silence errors with `any`, `@ts-ignore`, `@ts-expect-error`, `@ts-nocheck`, or by loosening tsconfig or excluding files. Keep runtime behavior the same as far as Solid 2 allows.

Work on your own until done; there is nobody to answer questions. You can read and edit files in this directory, read the guide, and run `pnpm` commands (use `pnpm exec tsc` for TypeScript); other commands and web access are not available. Don't commit or push. When you finish, print a short summary of what you changed and anything left.

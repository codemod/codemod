// Imported transforms in Node.js.
//
// A workflow imports the transforms it gives `jssg` and `shard`
// (`import codemod from "./scripts/codemod.ts"`), but never runs them: the
// engine does, file by file. The codemod CLI's build drops those imports and
// passes the engine references to the files. Here the workflow is loaded by
// Node.js itself, so a load hook does the same to the workflow file the host
// imports, through the native addon: the transform's module is not loaded.

import { fileURLToPath, pathToFileURL } from "node:url";

import { addon } from "./addon.mts";

/** Marks the import of a workflow file the host runs, for the load hook. */
const WORKFLOW_QUERY = "codemod-workflow";

/** What a load hook resolves to, as far as this file is concerned. */
interface Loaded {
  format?: string | null | undefined;
  source?: string | ArrayBuffer | ArrayBufferView | null | undefined;
}

/**
 * The URL the host imports a workflow file by: marked for the load hook,
 * where there is one (not in Bun).
 */
export function workflowUrl(path: string): string {
  const url = pathToFileURL(path).href;
  return (globalThis as { Bun?: unknown }).Bun === undefined ? `${url}?${WORKFLOW_QUERY}` : url;
}

/** `loaded` with its source rewritten when `url` is a workflow the host imports. */
export function loadWorkflow<T extends Loaded>(url: string, loaded: T): T {
  if (!url.startsWith("file:") || loaded.source == null) return loaded;
  if (!new URL(url).searchParams.has(WORKFLOW_QUERY)) return loaded;
  const native = addon();
  // An addon from before imported transforms: `jssg` explains at run time.
  if (typeof native.rewriteWorkflow !== "function") return loaded;
  const source =
    typeof loaded.source === "string"
      ? loaded.source
      : new TextDecoder().decode(loaded.source as Uint8Array);
  const rewritten = native.rewriteWorkflow(fileURLToPath(url), source);
  return rewritten === null ? loaded : { ...loaded, source: rewritten };
}

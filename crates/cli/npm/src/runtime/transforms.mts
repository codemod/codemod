// What the codemod build changes in a workflow, in Node.js.
//
// Node.js runs the workflow body and its callbacks in place, but the engine
// runs `shard`'s function and filter in its isolates, as with the CLI, and an
// imported callback's id covers its file and the files it imports. A load
// hook applies the build's changes for those to the workflow file the host
// imports, through the native addon.
//
// Workflows also import YAML files as data (`import rules from
// "./rules.yaml"`): the hook loads one as a module whose default export is
// its content, as the CLI's module loader does.

import { AsyncLocalStorage } from "node:async_hooks";
import { readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";

import { addon } from "./addon.mts";
import { context } from "./context.mts";

const readSource = readFileSync;
const loading = new AsyncLocalStorage<boolean>();
const runtimeDir = new URL("./", import.meta.url).href;

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

export function importWorkflow(path: string): Promise<unknown> {
  return loading.run(true, () => import(workflowUrl(path)));
}

export function refuseWorkflowFs(specifier: string, parent: string | undefined): void {
  if (
    (parent && new URL(parent).searchParams.has(WORKFLOW_QUERY)) ||
    ((loading.getStore() || context.getStore()) && !parent?.startsWith(runtimeDir))
  ) {
    if (/^(node:)?fs(\/promises)?$/.test(specifier)) {
      throw new Error(
        'fs is not available in 2.0 workflows; use import { files } from "codemod/files" inside a callback',
      );
    }
  }
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

/** Whether `url` is a YAML file, which Node.js cannot import by itself. */
export function isYamlUrl(url: string): boolean {
  return url.startsWith("file:") && /\.ya?ml$/i.test(new URL(url).pathname);
}

/** A YAML file as a module whose default export is its content. */
export function loadYaml(url: string): { format: "module"; source: string; shortCircuit: true } {
  const native = addon();
  if (typeof native.yamlModule !== "function") {
    throw new Error(
      "this codemod native addon cannot load YAML; reinstall codemod so the addon and the package match",
    );
  }
  const source = native.yamlModule(readSource(fileURLToPath(url), "utf8"));
  return { format: "module", source, shortCircuit: true };
}

// Loads the native addon (`codemod.node`): the workflow engine and the
// ast-grep API for code that runs in Node.js or Bun.
//
// Looked up in order: `CODEMOD_NODE_ADDON`, the platform package next to the
// CLI binary, then a cargo build in a checkout of the codemod repository.
//
// The interfaces here describe what the addon exports (crates/node/src).

import { existsSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import type { Json, Outcome, StepRecord, TestPlan } from "./orchestrator.mts";

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));

/** `OpenOptions` in crates/node/src/run.rs. */
export interface OpenOptions {
  workflow: string;
  target: string;
  params?: Record<string, Json>;
  name?: string;
  dryRun?: boolean;
  /** Stop after the step with this label (a test run). */
  until?: string;
  /** Results for steps that should not run, by label (a test run). */
  given?: Record<string, Json>;
  approve?: boolean;
  capabilities?: string[];
  store?: string;
  resume?: string;
  rerunSteps?: number[];
  skipSteps?: number[];
  decisionModel?: DecisionModelOptions;
}

/** `DecisionModelOptions` in crates/node/src/run.rs. */
export interface DecisionModelOptions {
  /** `"systemone"` or `"openai"`. */
  provider?: string;
  model?: string;
  apiKey?: string;
  /** The API root, for a proxy or a fake in tests. */
  baseUrl?: string;
}

/** An event as the engine reports it (`Event` in crates/workflow/src/events.rs). */
export type EngineEvent = { readonly event: string } & Record<string, unknown>;

/** What `Run.finish` returns: `RunReport::to_json`, with stats. */
export interface NativeReport {
  runId: string;
  status:
    | "succeeded"
    | "failed"
    | "canceled"
    | "paused"
    | "rejected"
    | "awaiting-approval"
    | "stopped";
  output: Json;
  error: string | null;
  awaiting: string | null;
  stoppedAt: string | null;
  /** The exit code the workflow gave `fail()`; `null` otherwise. */
  exitCode: number | null;
  steps: {
    position: number;
    label: string;
    status: "succeeded" | "partial" | "failed";
    result: Json;
    error: string | null;
    given: boolean;
  }[];
  stats: { filesModified: number; filesUnmodified: number; filesWithErrors: number };
}

/** What a `forEachFile` callback did to one file, for `Run.commitFiles`. */
export interface FileEffects {
  path: string;
  /** The file's text when the step read it. */
  original: string;
  effects: import("./orchestrator.mts").FileEffect[];
}

/** One run in the engine: `Run` in crates/node/src/run.rs. */
export interface NativeRun {
  readonly target: string;
  readonly params: Record<string, Json>;
  readonly dryRun: boolean;
  /** The run's id; the body's random numbers are seeded by it. */
  readonly runId: string;
  /** When the run started (ms since the epoch), also on a resume. */
  readonly startedAt: number;
  /** Steps recorded by earlier executions of the run. */
  history(): StepRecord[];
  testPlan(): TestPlan | null;
  /** Persist one step's outcome (a `StepRecord` as JSON). */
  record(entry: string, durationMs: number): void;
  /** Report a step started, replayed or retrying (a `StepEvent` as JSON). */
  event(event: string): void;
  /** A line the body (`null`) or the step at `position` wrote. */
  log(position: number | null, line: string): void;
  progress(position: number, processed: number, total: number): void;
  canceled(): boolean;
  /** Whether a step about to start must not, because the run was canceled. */
  refusesSteps(): boolean;
  cancel(): void;
  /** The test plan's `until` step has started. */
  stop(): void;
  /** Evaluate a workflow.yaml `${{ }}` expression; `scope` is JSON. */
  evaluate(kind: string, expression: string, scope: string): unknown;
  /** Run a host task (a `StartRequest` as JSON). Rejects with `{ name, message }` as JSON in the message. */
  runTask(request: string): Promise<Outcome>;
  /** The files a `forEachFile` selector (as JSON) picks. */
  selectFiles(
    selector: string,
  ): Promise<{ language: string | null; files: [path: string, text: string][] }>;
  /** Commit what a step's callbacks did to its files (`FileEffects[]` as JSON). */
  commitFiles(
    position: number,
    label: string,
    files: string,
  ): Promise<{ changes: Json; failures: { path: string; error: string }[]; assessed?: Json }>;
  /** Answer an `assess()` call of the step at `position`: `{ ok, value | error }` as JSON. */
  assess(position: number, request: string): Promise<string>;
  /** Events since the last call. */
  takeEvents(): EngineEvent[];
  /** Record how the run ended, given how its body ended (JSON, or `"null"`). */
  finish(body: string): NativeReport;
}

/** The addon's exports. */
export interface Addon {
  Run: { open(options: OpenOptions): NativeRun };
  /** Identifies the orchestrator source the addon's engine was built from. */
  orchestratorVersion(): string;
  /** The workflow file a path names: the file, or a package directory's workflow. */
  resolveWorkflow(source: string, name: string | null): string;
  /**
   * A workflow's source as Node.js loads it: `shard`'s functions as
   * references to the modules the build extracted, imported callbacks with
   * their ids; null when that changes nothing. Throws the build's error.
   */
  rewriteWorkflow?(file: string, source: string): string | null;
  /** A YAML file's text as an ES module whose default export is its content. */
  yamlModule?(text: string): string;
  /** Parse `source` as `language`; returns an `SgRoot`. */
  parse(language: string, source: string, filename?: string, relativeFilename?: string): unknown;
  languageFromPath(path: string): string | null;
  kind(language: string, kindName: string): number;
}

/** What the `codemod` launcher next to this package exports. */
interface Launcher {
  detectPackageName(): string | null | undefined;
}

let loaded: Addon | undefined;

/** The addon's exports. Throws with what to do when it cannot be found. */
export function addon(): Addon {
  if (loaded) return loaded;
  const path = addonPath();
  const module = { exports: {} };
  process.dlopen(module, path);
  loaded = module.exports as Addon;
  return loaded;
}

/** Where the addon is, or throws. */
export function addonPath(): string {
  if (process.env.CODEMOD_NODE_ADDON) return process.env.CODEMOD_NODE_ADDON;
  const { detectPackageName } = require("../codemod") as Launcher;
  const pkgName = detectPackageName();
  if (pkgName) {
    try {
      const candidate = join(
        dirname(require.resolve(`${pkgName}/package.json`, { paths: [here] })),
        "codemod.node",
      );
      if (existsSync(candidate)) return candidate;
    } catch {
      // not installed
    }
  }
  const library =
    process.platform === "win32"
      ? "codemod_node.dll"
      : process.platform === "darwin"
        ? "libcodemod_node.dylib"
        : "libcodemod_node.so";
  // In a checkout: whichever cargo build is newest, so a stale one is not
  // picked over the build just made.
  const [newest] = ["release", "debug"]
    .map((profile) => join(here, "..", "..", "..", "..", "target", profile, library))
    .filter((candidate) => existsSync(candidate))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  if (newest) return newest;
  throw new Error(
    `cannot find the codemod native addon${pkgName ? ` (expected codemod.node in the ${pkgName} package)` : " for this platform"}; reinstall codemod with optional dependencies, or set CODEMOD_NODE_ADDON`,
  );
}

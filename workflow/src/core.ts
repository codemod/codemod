// The roaster's analysis core, bundled into workflow.ts by build.mjs.
// Slices are imported unchanged; only the parse adapter and engine glue are new.
import type {
  EngineState,
  FileContext,
  Finding,
  JsonValue,
  ReportSection,
  SourceLanguage,
  SourceRange,
  SyntaxNode,
} from "../../src/engine/types.ts";
import { allSlices } from "../../src/slices/all-slices.ts";

// The structural surface of a codemod:ast-grep node that the slices read.
type SgNode = {
  kind(): unknown;
  isNamed(): boolean;
  text(): string;
  range(): SourceRange;
  children(): SgNode[];
  field(name: string): SgNode | null;
  findAll(matcher: { rule: { kind: string } }): SgNode[];
};

const LANGUAGES: { [extension: string]: SourceLanguage } = {
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  rs: "rust",
};

export const SOURCE_GLOBS = Object.keys(LANGUAGES).map((extension) => `**/*.${extension}`);
export const GENERATED_GLOBS = ["node_modules", "dist", "build", "coverage"].flatMap(
  (directory) => [`${directory}/**`, `**/${directory}/**`],
);

function readonlyNode(node: SgNode): SyntaxNode {
  return {
    kind: () => String(node.kind()),
    isNamed: () => node.isNamed(),
    text: () => node.text(),
    range: () => node.range(),
    children: () => node.children().map(readonlyNode),
    field: (name) => {
      const child = node.field(name);
      return child ? readonlyNode(child) : null;
    },
    findAll: (kind) => node.findAll({ rule: { kind } }).map(readonlyNode),
  };
}

/** Slice families, by rule id prefix: one workflow step each. */
export const FAMILIES = [...new Set(allSlices.map((slice) => slice.id.split("/")[0] ?? ""))].sort();

/** Runs the family's slices (all slices when omitted) over one parsed file. */
export function analyzeFile(path: string, root: SgNode, family?: string): EngineState {
  const extension = path.split(".").pop() ?? "";
  const file: FileContext = {
    path,
    language: LANGUAGES[extension] ?? "typescript",
    root: readonlyNode(root),
  };
  const state: EngineState = {};
  for (const slice of allSlices) {
    if (family === undefined || slice.id.startsWith(`${family}/`)) state[slice.id] = slice.analyze(file);
  }
  return state;
}

/** Folds per-file states into one, slice by slice, with each slice's own merge. */
export function mergeStates(states: readonly EngineState[]): EngineState {
  const merged: EngineState = {};
  for (const slice of allSlices) {
    let state: JsonValue = slice.initialState();
    for (const fileState of states) {
      state = slice.merge(state, fileState[slice.id] ?? slice.initialState());
    }
    merged[slice.id] = state;
  }
  return merged;
}

export function reportSections(state: EngineState): ReportSection[] {
  return [...allSlices]
    .sort((left, right) => compareText(left.id, right.id))
    .flatMap((slice) => {
      const section = slice.report(state[slice.id] ?? slice.initialState());
      return section ? [{ ...section, findings: [...section.findings].sort(compareFinding) }] : [];
    });
}

/** Same text as the CLI's renderer; `quips` maps a rule id to the chosen quip. */
export function renderReport(
  sections: readonly ReportSection[],
  quips: { [ruleId: string]: string },
): string {
  const heading = "CODEBASE ROASTER — syntax hotspots";
  if (sections.length === 0) return `${heading}\n\nClean run: no enabled pattern matched.`;
  return [
    heading,
    ...sections.map((section) =>
      [
        `[${section.ruleId}] ${section.title}`,
        `Fact: ${section.summary}`,
        "Evidence:",
        ...section.findings.map(
          (finding) => `- ${finding.path}:${finding.line}:${finding.column} — ${finding.fact}`,
        ),
        `Remediation: ${section.remediation}`,
        `Roast: ${quips[section.ruleId]}`,
      ].join("\n"),
    ),
  ].join("\n\n");
}

function compareFinding(left: Finding, right: Finding): number {
  return (
    compareText(left.path, right.path) ||
    left.line - right.line ||
    left.column - right.column ||
    compareText(left.fact, right.fact)
  );
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

/** Drops slices whose file state is still the initial one, so recorded outputs stay small. */
export function nonEmptyState(state: EngineState): EngineState | undefined {
  const kept: EngineState = {};
  for (const slice of allSlices) {
    const value = state[slice.id];
    if (value === undefined) continue;
    if (JSON.stringify(value) !== JSON.stringify(slice.initialState())) {
      kept[slice.id] = value;
    }
  }
  return Object.keys(kept).length > 0 ? kept : undefined;
}

export function renderWarnings(failures: readonly { path: string; error: string }[]): string {
  if (failures.length === 0) return "";
  return [
    "Coverage warnings:",
    ...[...failures]
      .sort((left, right) => compareText(left.path, right.path))
      .map((failure) => `- ${failure.path} [parse] ${failure.error}`),
  ].join("\n");
}

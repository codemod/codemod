import { forEachFile, step } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
import {
  FAMILIES,
  GENERATED_GLOBS,
  SOURCE_GLOBS,
  analyzeFile,
  mergeStates,
  nonEmptyState,
  renderReport,
  renderWarnings,
  reportSections,
} from "./core.ts";

export default async function main() {
  // One step per family, run together: each family re-parses every file.
  const scans = await Promise.all(
    ["all"].map((family) =>
      step(
        `Analyze ${family}`,
        forEachFile({ include: SOURCE_GLOBS, exclude: GENERATED_GLOBS }, (file) => {
          const state = nonEmptyState(analyzeFile(file.path, parse(file).root()));
          return state ? { path: file.path, state } : undefined;
        }),
      ),
    ),
  );

  const sections = reportSections(
    mergeStates(scans.flatMap((scan) => scan.outputs.map((output) => output.state))),
  );
  // Math.random in the body is seeded by the run, so a resume replays the same roasts.
  const quips = Object.fromEntries(
    sections.map((section) => [
      section.ruleId,
      section.quips[Math.floor(Math.random() * section.quips.length)],
    ]),
  );
  // Every family parses the same files, so one family's failures cover them all.
  const failures = scans[0]?.failures ?? [];
  console.log(
    [renderReport(sections, quips), renderWarnings(failures)].filter(Boolean).join("\n\n"),
  );
  return {
    findings: Object.fromEntries(sections.map((section) => [section.ruleId, section.findings.length])),
    unparsed: failures.map((failure) => failure.path),
  };
}

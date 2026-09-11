import { guard, jssg, workflow } from "../../../src/index.ts";
import { migrateText, posixPath } from "./helpers.ts";

interface Finding {
  file: string;
}

const Findings = guard(
  "Findings",
  (value: unknown): value is Finding[] =>
    Array.isArray(value) &&
    value.every(
      (item) =>
        typeof item === "object" &&
        item !== null &&
        typeof (item as { file?: unknown }).file === "string",
    ),
);

// The transform is written inline; `codemod-workflow` bundles it (with the
// helpers it imports) into a standalone artifact before this module runs.
const migrate = jssg({
  name: "migrate",
  language: "typescript",
  include: ["**/*.ts"],
  exclude: ["**/*.d.ts"],
  semanticAnalysis: "workspace",
  selector: { rule: { pattern: "oldApi($ARG)" } },
  output: Findings,
  transform(root) {
    return {
      content: migrateText(root.root().text()),
      output: { file: posixPath(root.relativeFilename()) },
    };
  },
});

export default workflow(() =>
  migrate({
    target: { include: ["src/**"], exclude: ["**/*.generated.ts"] },
  }),
);

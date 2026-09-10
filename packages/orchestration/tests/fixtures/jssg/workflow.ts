import { guard, jssg, workflow } from "../../../src/index.ts";

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

// `script` is relative to the executor's script root, which the local CLI
// sets to this workflow's directory.
const migrate = jssg<void, Finding[]>({
  name: "migrate",
  script: "transform.ts",
  language: "typescript",
  include: ["**/*.ts"],
  exclude: ["**/*.d.ts"],
  semanticAnalysis: "workspace",
  output: Findings,
});

export default workflow(() =>
  migrate({
    target: { include: ["src/**"], exclude: ["**/*.generated.ts"] },
  }),
);

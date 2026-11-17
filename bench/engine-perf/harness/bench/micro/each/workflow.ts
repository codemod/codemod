import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  const r = await step("scan", forEachFile({ include: ["**/*.ts", "**/*.tsx"], exclude: ["**/node_modules/**", "**/*.d.ts"], language: "tsx" },
    (file) => parse(file).root().findAll({ rule: { kind: "call_expression" } }).length));
  console.log(`MICRO files=${r.outputs.length} failures=${r.failures.length}`);
}

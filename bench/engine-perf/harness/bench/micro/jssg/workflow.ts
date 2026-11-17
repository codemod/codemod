import { step, forEachFile, jssg } from "codemod/workflow";
const files = { include: ["**/*.ts", "**/*.tsx"], exclude: ["**/node_modules/**", "**/*.d.ts"], language: "tsx" };
export default async function main() {
  const r = await step("scan", forEachFile(files, jssg("./scan.ts", { semanticAnalysis: "none" })));
  console.log(`MICRO files=${r.unmodified + r.changes.written.length} failures=${r.failures.length}`);
}

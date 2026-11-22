// A shard filter that type-tests/workflow.ts imports.

import type { WorkflowFile } from "codemod/workflow";

export default function wouldChange(file: WorkflowFile): boolean {
  return file.text.includes("var ");
}

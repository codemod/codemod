import type { Codemod, GetSelector } from "codemod/ast-grep";
import type TS from "codemod/ast-grep/langs/typescript";

export const getSelector: GetSelector<TS> = () => ({
  rule: { pattern: "console.log($$$)" },
});

const transform: Codemod<TS> = async (root, options) => {
  const found = options.matches?.length ?? 0;
  return { edit: null, data: { file: root.filename(), found, mode: options.params.mode ?? "" } };
};

export default transform;

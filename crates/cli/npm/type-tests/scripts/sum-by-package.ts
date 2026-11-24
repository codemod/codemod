import type { FileReducer } from "codemod/workflow";

/** Adds each file's number into its package's total. */
const sumByPackage: FileReducer<Record<string, number>, number> = (totals, size, path) => {
  const name = path.split("/")[1] ?? ".";
  return { ...totals, [name]: (totals[name] ?? 0) + size };
};
export default sumByPackage;

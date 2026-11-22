// LLRT declares these two named exports but only sets them on the default
// object (as non-enumerable properties). Preserve the rest of its API.
import path from "node:path";

export * from "node:path";
export default path;
export const { sep, delimiter } = path;

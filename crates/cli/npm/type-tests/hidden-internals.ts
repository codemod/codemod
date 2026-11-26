// @ts-expect-error The YAML expression evaluator is not a public workflow API.
import type { __evaluate } from "codemod/workflow";
// @ts-expect-error The translator's JSSG helpers have no public module types.
import type { jssgFile, jssgWouldChange } from "codemod-internal:jssg-compat";

export type PrivateHelpers = [typeof __evaluate, typeof jssgFile, typeof jssgWouldChange];

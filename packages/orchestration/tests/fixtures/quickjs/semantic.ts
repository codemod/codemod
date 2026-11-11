// JSSG semantic analysis in-process: workspace scope (the index covers files
// the selector skips; `write()` stages a cross-file edit), then file scope.
import { dynamic } from "../../../src/index.ts";
import { definitionKind, semantic } from "../jssg/transforms.ts";

export default dynamic(async () => [await semantic(), await definitionKind()]);

import type { Transform } from "codemod:ast-grep";
import type TS from "codemod:ast-grep/langs/typescript";

// Module-level state: with a fresh runtime per file this is always 1;
// with a reused runtime it counts files this thread has already seen.
let seen = 0;
const transform: Transform<TS> = (root) => {
  seen += 1;
  return `// seen=${seen}\n${root.root().text()}`;
};
export default transform;

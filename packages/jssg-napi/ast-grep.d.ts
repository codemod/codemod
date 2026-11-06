// The runtime `codemod:ast-grep` module; its full types are `codemod:ast-grep`
// in `@codemod.com/jssg-types`.
export const SgRoot: abstract new () => object;
export const SgNode: abstract new () => object;
export function parse(language: string, source: string): object;
export function parseAsync(language: string, source: string): object;
export function parseFile(language: string, path: string): object;
export function kind(language: string, kind: string): number;
export function jssgTransform(
  transform: (root: object, options: object) => unknown,
  path: string,
  language: string,
): Promise<string | null>;
declare const astGrep: {
  SgRoot: typeof SgRoot;
  SgNode: typeof SgNode;
  parse: typeof parse;
  parseAsync: typeof parseAsync;
  parseFile: typeof parseFile;
  kind: typeof kind;
  jssgTransform: typeof jssgTransform;
};
export default astGrep;

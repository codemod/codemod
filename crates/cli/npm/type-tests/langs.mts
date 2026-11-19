// A workflow in an ES module project (`"type": "module"`, or a .mts file)
// imports a language's types as a type, and parses with it.
import { parse } from "codemod/ast-grep";
import type Python from "codemod/ast-grep/langs/python";
import type TS from "codemod/ast-grep/langs/typescript";
import type TSX from "codemod/langs/tsx";

const root = parse<TS>("const a = 1;", { language: "typescript" });
const declaration = root.root().find({ rule: { kind: "lexical_declaration" } });
export const kind: string | undefined = declaration?.kind();

export type Languages = [Python, TSX];

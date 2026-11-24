import { assess, choice, yesNo } from "codemod/assess";

export const asking = (path: string) =>
  assess(path, { questions: { ok: yesNo("Is it fine?"), kind: choice(["a", "b"]) } });

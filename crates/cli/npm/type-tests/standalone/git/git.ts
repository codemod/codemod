import { branch, commit, pullRequest } from "codemod/git";

export const tasks = [branch("codemod-x"), commit("x"), pullRequest({ title: "x" })];

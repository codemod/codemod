import { dependencies, install } from "codemod/deps";

export const task = install();
export const read = () => dependencies().packages().length;

import { promises } from "codemod-internal:fs-compat";
export const { access, stat, readFile, writeFile, readdir, mkdir, unlink, constants } = promises;
export default promises;

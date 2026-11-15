// A workflow finds its own files through import.meta, as in Node.js.
import { shell, step } from "codemod/workflow";

const dirname: string = import.meta.dirname;
const filename: string = import.meta.filename;
const url: string = import.meta.url;

export default async function main() {
  await step("Setup", shell`${dirname}/scripts/setup.sh`);
  return { filename, url };
}

// `child_process` in steps' and callbacks' isolates: importing it works, so a
// package that only mentions it loads, and calling anything in it fails with
// the step to use instead, `shell()`.

import { unavailable } from "codemod-internal:orchestrator";

function refuse(name: string): (...args: unknown[]) => never {
  return () => {
    throw new Error(unavailable(`child_process.${name}`, "callback"));
  };
}

export const exec = refuse("exec");
export const execSync = refuse("execSync");
export const execFile = refuse("execFile");
export const execFileSync = refuse("execFileSync");
export const spawn = refuse("spawn");
export const spawnSync = refuse("spawnSync");
export const fork = refuse("fork");

export default { exec, execSync, execFile, execFileSync, spawn, spawnSync, fork };

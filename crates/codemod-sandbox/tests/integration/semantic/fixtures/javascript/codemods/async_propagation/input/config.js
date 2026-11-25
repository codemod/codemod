import { readFileSync } from "node:fs";

export function loadConfig(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

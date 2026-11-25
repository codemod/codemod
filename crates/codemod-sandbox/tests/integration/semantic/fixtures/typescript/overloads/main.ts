import { pick, useAmbient } from "./lib";

export function main() {
  pick("a");
  useAmbient();
}

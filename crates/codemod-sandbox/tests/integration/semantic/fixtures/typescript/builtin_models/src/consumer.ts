import { a } from "./lib";

export const loadB = () => import("./lib").then((m) => m.b + a);

export async function loadB2() {
  const { b } = await import("./lib");
  return b;
}

export async function loadB3() {
  const mod = await import("./lib");
  return mod.b;
}

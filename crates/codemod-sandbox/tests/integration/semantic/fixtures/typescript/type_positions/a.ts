export function f(n: number) {
  return n;
}

export type F = typeof import("./a").f;

export const value = f(1) as number;

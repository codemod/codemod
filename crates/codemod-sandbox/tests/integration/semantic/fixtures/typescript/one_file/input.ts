const x = 1;
const y = x + 2;
console.log(x);

function greet(name: string): string {
  return "Hello, " + name;
}

const message1 = greet("Alice");
const message2 = greet("Bob");
console.log(greet("World"));

const typed: string[] = ["x"];
const typedStr: string = "z";
function echo(value: string) {
  return value;
}
echo("w");
console.log(typed, typedStr);

export function f(n: number) {
  return n;
}

export type F = typeof import("./input").f;

export const value = f(1) as number;

import { normalize, query } from "./lib";

function pass(x: unknown) {
  return x;
}

export function handler(req) {
  const name = normalize(req.body.name);
  query("select " + name, 10);
  const other = normalize("literal");
  log(other);
  pass(5);
  pass(name);
}

import { audit, first, identity, later, rest, save, wrap } from "./lib";

function local(text: string) {
  return text.trim();
}

export function fromA(req) {
  const a = identity(req.body.a);
  sink(a);
}

export function fromB() {
  const b = identity("b-literal");
  log(b);
}

export function passes(user) {
  audit(user);
  const kept = first(user, 1);
  const dropped = first(1, user);
  const boxed = wrap(user);
  save(user);
  const read = later(user);
  const spread = rest(1, user);
  const trimmed = local(user);
}

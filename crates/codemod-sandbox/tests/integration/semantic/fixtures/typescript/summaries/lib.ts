export const store: { last?: unknown } = {};

export function identity(value: unknown) {
  return value;
}

export function audit(user: { id: string }) {
  console.log(user.id);
  return true;
}

export function first(a: unknown, b: unknown) {
  return a;
}

export function wrap(value: unknown) {
  const box: { value?: unknown } = {};
  box.value = value;
  return box;
}

export function save(item: unknown) {
  store.last = item;
}

export function later(value: unknown) {
  const read = () => value;
  return read();
}

export function rest(...values: unknown[]) {
  return values;
}

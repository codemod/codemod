import { toMoment, deepFn, renamed, viaImport } from "@demo/shared";

export function render(d: Date) {
  toMoment(d);
  deepFn();
  renamed();
  viaImport(d);
}

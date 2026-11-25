import { registry } from "../registry";
export function Icon(name: string) {
  return registry.get(name);
}

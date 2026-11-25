import { format } from "./lib.js";
import Db from "./db.js";

export function render(value) {
  return new Db(format(value));
}

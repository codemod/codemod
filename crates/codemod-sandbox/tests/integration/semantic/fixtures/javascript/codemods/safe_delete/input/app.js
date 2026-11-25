import { formatPrice, viaHelper } from "./utils.js";
import * as utils from "./utils.js";

export function render(order) {
  return formatPrice(order.total) + viaHelper(order.count);
}

// Stored through the namespace, never called here: no callers, but referenced.
export const handlers = { round: utils.oldRound };

// Handed to code the analysis cannot see, through the namespace.
hooks.register(utils.registered);

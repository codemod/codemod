import { formatPrice, viaHelper, oldRound, registered } from "./utils.js";

export function render(order) {
  return formatPrice(order.total) + viaHelper(order.count);
}

// Stored, never called here: no callers, but it escapes as a value.
export const handlers = { round: oldRound };

// Handed to code the analysis cannot see: assumed called.
hooks.register(registered);

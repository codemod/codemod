export function formatPrice(cents) {
  return "$" + (cents / 100).toFixed(2);
}

export function legacyFormat(cents) {
  return cents + "c";
}

export function oldRound(n) {
  return Math.round(n);
}

export function registered(n) {
  return n * 2;
}

function internalHelper(n) {
  return n + 1;
}

export function viaHelper(n) {
  return internalHelper(n);
}

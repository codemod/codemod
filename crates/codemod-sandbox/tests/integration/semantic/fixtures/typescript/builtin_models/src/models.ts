// Builtin method models: values reach inline callbacks' parameters, and
// what the callbacks return makes the call's result.
export function totals(orders, fetchRate) {
  const amounts = orders.map((order) => order.amount);
  const big = orders.filter((o) => o.amount > 100);
  const sum = amounts.reduce((acc, value) => acc + value, 0);
  big.forEach((o) => audit(o.id));
  fetchRate().then(apply);
  return fetchRate()
    .then((rate) => send(sum * rate))
    .catch((err) => report(err));
}

function apply(rate) {
  record(rate);
}

export const loadC = () => import("./lib/c").then(({ c }) => c());

export async function runC() {
  const mod = await import("./lib/c");
  return mod.c();
}

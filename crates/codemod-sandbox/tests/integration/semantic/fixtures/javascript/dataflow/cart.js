export function checkout(cart, coupons) {
  const items = cart.items;
  items.push({ sku: "gift" });
  const total = sum(items);
  const snapshot = items;
  audit(total);
  snapshot.sort(byPrice);
  let count = items.length;
  count = count + coupons.length;
  setTimeout(() => {
    items.length = 0;
  }, 0);
  return { total, count };
}

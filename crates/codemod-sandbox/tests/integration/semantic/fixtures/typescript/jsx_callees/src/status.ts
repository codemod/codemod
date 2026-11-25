export function statusLabel(status: string) {
  if (status === "pending") return "Waiting for approval";
  if (status === "shipped") return "On its way";
  return "Delivered";
}

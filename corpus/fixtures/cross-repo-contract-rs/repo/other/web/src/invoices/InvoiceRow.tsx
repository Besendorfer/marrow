export function invoiceLabel(state: "draft" | "open" | "paid"): string {
  return state === "paid" ? "Paid" : "Due";
}

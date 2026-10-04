import type { Invoice } from "./types";

export function renderInvoice(id: string, store: Map<string, Invoice>): string {
  const inv = store.get(id);
  if (!inv) return "<p>Invoice not found</p>";
  return `<section>${formatInvoice(inv)}</section>`;
}

export function formatInvoice(inv: Invoice | null): string {
  if (!inv) return "";
  let out = `<h2>Invoice ${inv.number}</h2>`;
  for (let i = 0; i < inv.lines.length; i++) {
    const line = inv.lines[i];
    out += `<p>${line.label}: ${line.amount.toFixed(2)}</p>`;
  }
  return out;
}

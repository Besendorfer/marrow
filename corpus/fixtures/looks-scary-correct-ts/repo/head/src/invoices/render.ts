import type { Invoice } from "./types";

export function renderInvoice(id: string, store: Map<string, Invoice>): string {
  const inv = store.get(id);
  if (!inv) return "<p>Invoice not found</p>";
  return `<section>${formatInvoice(inv)}</section>`;
}

// Only renderInvoice calls this, after its own not-found check, so the
// invoice is never null here; the type now says so.
export function formatInvoice(inv: Invoice): string {
  let out = `<h2>Invoice ${inv.number}</h2>`;
  for (const line of inv.lines) {
    out += `<p>${line.label}: ${line.amount.toFixed(2)}</p>`;
  }
  return out;
}

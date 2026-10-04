export interface InvoiceLine {
  label: string;
  amount: number;
}

export interface Invoice {
  number: string;
  lines: InvoiceLine[];
}

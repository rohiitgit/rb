// Pays an invoice through the billing API; `base` is the deployment's billing URL.
export function payInvoice(base: string, id: string) {
  return fetch(`${base}/invoices/${id}/pay`, { method: "POST" });
}

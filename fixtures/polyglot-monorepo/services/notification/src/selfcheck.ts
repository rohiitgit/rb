// Health probe run inside the container: posts a dry-run email request to this process.
export async function selfcheck(): Promise<boolean> {
  const res = await fetch("http://localhost:3000/internal/mfa/email", { method: "POST" });
  return res.ok;
}

// The SMS route answers 501 until a provider is configured; the probe expects exactly that.
export async function smsCheck(): Promise<boolean> {
  const res = await fetch("/internal/mfa/sms", { method: "POST" });
  return res.status === 501;
}

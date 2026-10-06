// Health probe run inside the container: posts a dry-run email request to this process.
export async function selfcheck(): Promise<boolean> {
  const res = await fetch("http://localhost:3000/internal/mfa/email", { method: "POST" });
  return res.ok;
}

export async function loginViaNginx() {
  await fetch("http://gateway/api/v1/login", { method: "POST" });
}

export async function loginViaPay() {
  await fetch("http://gateway/pay/v1/login", { method: "POST" });
}

export async function loginViaBill() {
  await fetch("http://gateway/bill/v1/login", { method: "POST" });
}

export async function loginViaAlias() {
  await fetch("http://login/v1/login", { method: "POST" });
}

export async function loginViaK8s() {
  await fetch("http://auth-http/v1/login", { method: "POST" });
}

// Plain GET for URLs the caller builds at runtime.
export function get(url: string) {
  return fetch(url);
}

// The hosting provider's public status page, which mirrors our login probe.
export function status() {
  return fetch("https://status.example.com/api/v1/login");
}

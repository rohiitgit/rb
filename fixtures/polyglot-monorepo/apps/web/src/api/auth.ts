import axios from "axios";
import type { Credentials } from "@acme/shared";

const API = import.meta.env.VITE_API_URL ?? "http://gateway";

export async function login(creds: Credentials): Promise<string> {
  const { data } = await axios.post(`${API}/api/v1/login`, creds);
  return data.token;
}

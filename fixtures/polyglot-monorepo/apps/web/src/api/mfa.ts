import axios from "axios";

// Asks for the login code to be sent again; relative to the page's origin.
export function resend(email: string) {
  return axios.post("/internal/mfa/email", { email });
}

import axios from "axios";
import { extOpts } from "./opts";

// Asks for the login code to be sent again; relative to the page's origin.
export function resend(email: string) {
  return axios.post("/internal/mfa/email", { email });
}

const mailCfg = { baseURL: "http://notification" };

// Contract M6b S5: a config passed by name reads the object it names, so
// the service base restricts the match to notification's endpoint alone.
export function resendViaCfg(email: string) {
  return axios.post("/internal/mfa/email", { email }, mailCfg);
}

// Contract M6b S5: a spread from an imported const reads its `baseURL`
// through the unique declaration; the external candidate takes the site
// out of stitching entirely.
export function resendExternal(email: string) {
  return axios.post("/internal/mfa/email", { email }, { ...extOpts });
}

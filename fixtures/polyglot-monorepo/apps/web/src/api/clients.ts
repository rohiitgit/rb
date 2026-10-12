import got from "got";
import ky from "ky";
import request from "superagent";
import { request as undiciRequest } from "undici";
import { HttpClient } from "@angular/common/http";
import axios from "axios";

// Contract M6b S9: fetch-class libraries stitch like fetch with hook proofs
// (heuristic here, not synthetic: a `fetch` word in broken/pom.xml caps
// fetch repo-wide).
export async function fetchHealthViaGot() {
  return got.get("http://notification/health");
}

export async function sendMailViaGot(email: string) {
  return got.post("http://notification/internal/mfa/email", { json: { email } });
}

// A configured carrier never rises above heuristic.
const mailApi = got.extend({ prefixUrl: "http://notification" });

export async function sendMailViaGotExtended(email: string) {
  return mailApi.post("http://notification/internal/mfa/email", { json: { email } });
}

export async function fetchHealthViaKy() {
  return ky.get("http://notification/health");
}

const kyMailApi = ky.create({ prefixUrl: "http://notification" });

export async function sendMailViaKyExtended(email: string) {
  return kyMailApi.post("http://notification/internal/mfa/email", { json: { email } });
}

export async function fetchHealthViaSuperagent() {
  return request.get("http://notification/health");
}

const agent = request.agent();

export async function sendMailViaAgent(email: string) {
  return agent.post("http://notification/internal/mfa/email").send({ email });
}

export async function fetchHealthViaUndici() {
  return undiciRequest("http://notification/health");
}

// Angular HttpClient calls enter capped at heuristic (contract M6b S9, D8).
export class MailClient {
  constructor(private http: HttpClient) {}
  send(email: string) {
    return this.http.post("http://notification/internal/mfa/email", { email });
  }
}

// A destructured axios verb enters capped at heuristic (contract M6b S9).
const { post } = axios;

export async function resendViaMember(email: string) {
  return post("http://notification/internal/mfa/email", { email });
}

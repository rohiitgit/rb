import express from "express";
import { Pool } from "pg";

const app = express();
const pool = new Pool();

app.post("/internal/mfa/email", async (req, res) => {
  await pool.query("INSERT INTO email_log (recipient) VALUES ($1)", [req.body.email]);
  res.status(202).end();
});
// No SMS provider is configured yet; callers get a clear answer instead of a timeout.
app.post("/internal/mfa/sms", (_req, res) => {
  res.status(501).end();
});
// app.post("/internal/mfa/push", async (req, res) => { res.status(202).end(); });
app.listen(3000);

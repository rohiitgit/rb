import express from "express";
import { Pool } from "pg";

const app = express();
const pool = new Pool();

app.post("/internal/mfa/email", async (req, res) => {
  await pool.query("INSERT INTO email_log (recipient) VALUES ($1)", [req.body.email]);
  res.status(202).end();
});
app.listen(3000);

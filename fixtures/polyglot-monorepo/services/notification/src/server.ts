import express from "express";

const app = express();
app.post("/internal/mfa/email", (_req, res) => res.status(202).end());
app.listen(3000);

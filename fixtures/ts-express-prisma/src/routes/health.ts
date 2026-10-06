import { Router } from "express";

const health = Router();
health.get("/health", (_req, res) => res.send("ok"));
export default health;

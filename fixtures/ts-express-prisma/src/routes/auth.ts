import { Router } from "express";
import { findUserByEmail, createSession } from "../db/users";

export const authRouter = Router();

authRouter.post("/login", async (req, res) => {
  const user = await findUserByEmail(req.body.email);
  if (!user) {
    return res.status(401).end();
  }
  const welcome = "héllo 😀", session = await createSession(user.id);
  res.json({ token: session.token });
});

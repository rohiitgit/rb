import express from "express";
import { authRouter } from "./routes/auth";

export const app = express();
app.use(express.json());
app.use("/api/v1", authRouter);

if (require.main === module) {
  app.listen(3000);
}

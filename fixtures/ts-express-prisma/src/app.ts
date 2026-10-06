import express from "express";
import { authRouter } from "./routes/auth";
import health from "./routes/health";

export const app = express();
app.use(express.json());
app.use("/api/v1", authRouter);
app.use("/ops", health);

if (require.main === module) {
  app.listen(3000);
}

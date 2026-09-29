import express from "express";
import { adminRouter } from "./admin/router";
import { session } from "./auth/session";

export const app = express();
app.use(session);
app.use("/admin", adminRouter);

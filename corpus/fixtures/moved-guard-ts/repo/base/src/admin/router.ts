import { Router } from "express";
import { deleteUser, listUsers, resetPassword } from "./users";

export const adminRouter = Router();

adminRouter.post("/users/:id/reset-password", resetPassword);
adminRouter.get("/users", listUsers);
adminRouter.delete("/users/:id", deleteUser);

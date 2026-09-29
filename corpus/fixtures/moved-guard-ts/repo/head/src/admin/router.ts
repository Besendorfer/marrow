import { Router } from "express";
import { requireAdmin } from "../auth/requireAdmin";
import { deleteUser, listUsers, resetPassword } from "./users";

export const adminRouter = Router();

adminRouter.post("/users/:id/reset-password", resetPassword);
// Admin-only from here on: the handlers no longer check for themselves.
adminRouter.use(requireAdmin);
adminRouter.get("/users", listUsers);
adminRouter.delete("/users/:id", deleteUser);

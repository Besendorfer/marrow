import type { Request, Response } from "express";
import { db } from "../db";

export async function listUsers(req: Request, res: Response) {
  res.json(await db.users.list());
}

export async function resetPassword(req: Request, res: Response) {
  await db.users.resetPassword(req.params.id);
  res.status(204).end();
}

export async function deleteUser(req: Request, res: Response) {
  await db.users.delete(req.params.id);
  res.status(204).end();
}

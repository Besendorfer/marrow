import type { Request, Response } from "express";
import { db } from "../db";

export async function listUsers(req: Request, res: Response) {
  if (!req.user?.isAdmin) return res.status(403).end();
  res.json(await db.users.list());
}

export async function resetPassword(req: Request, res: Response) {
  if (!req.user?.isAdmin) return res.status(403).end();
  await db.users.resetPassword(req.params.id);
  res.status(204).end();
}

export async function deleteUser(req: Request, res: Response) {
  if (!req.user?.isAdmin) return res.status(403).end();
  await db.users.delete(req.params.id);
  res.status(204).end();
}

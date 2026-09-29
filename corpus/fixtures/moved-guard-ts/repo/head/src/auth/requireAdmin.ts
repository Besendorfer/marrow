import type { NextFunction, Request, Response } from "express";

/** Rejects any request whose user isn't an admin. */
export function requireAdmin(req: Request, res: Response, next: NextFunction) {
  if (!req.user?.isAdmin) return res.status(403).end();
  next();
}

// Checks handlers (moved from App.tsx in issue #238 phase 2; the CI-modal
// dismissal went with the modal in phase 5).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke } from "@tauri-apps/api/core";
import type { PrChecksStatus } from "../types";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createChecks(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const { setChecksMap } = ctx;

  async function fetchChecksStatus(tabId: string, prUrl: string) {
    try {
      const checks = await invoke<PrChecksStatus>("get_pr_checks", { prUrl });
      setChecksMap((prev) => ({ ...prev, [tabId]: checks }));
    } catch {
      // Non-critical: if fetching fails, don't block the review
    }
  }

  return { fetchChecksStatus };
}

export type ChecksApi = ReturnType<typeof createChecks>;

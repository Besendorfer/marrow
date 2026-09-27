// Checks handlers (moved verbatim from App.tsx in issue #238 phase 2).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke } from "@tauri-apps/api/core";
import type { PrChecksStatus } from "../types";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createChecks(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const { setChecksMap, setChecksDismissed } = ctx;

  async function fetchChecksStatus(tabId: string, prUrl: string) {
    try {
      const [checks, dismissed] = await Promise.all([
        invoke<PrChecksStatus>("get_pr_checks", { prUrl }),
        invoke<boolean>("is_checks_dismissed", { prUrl }),
      ]);
      setChecksMap((prev) => ({ ...prev, [tabId]: checks }));
      if (dismissed) {
        setChecksDismissed((prev) => ({ ...prev, [prUrl]: true }));
      }
    } catch {
      // Non-critical: if fetching fails, don't block the review
    }
  }

  async function handleDismissChecks(prUrl: string) {
    setChecksDismissed((prev) => ({ ...prev, [prUrl]: true }));
    try {
      await invoke("dismiss_checks_warning", { prUrl });
    } catch {
      // Persistence failure is non-critical
    }
  }

  return { fetchChecksStatus, handleDismissChecks };
}

export type ChecksApi = ReturnType<typeof createChecks>;

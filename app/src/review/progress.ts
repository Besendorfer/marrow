// Progress handlers (moved verbatim from App.tsx in issue #238 phase 2;
// checked-findings handlers added in phase 3).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke } from "@tauri-apps/api/core";
import type { ReviewManifest, Tab, ViewedFileState, NoteResolution, CheckedFindingEntry } from "../types";
import type { Finding } from "./findings";
import { parsePrUrl } from "../utils";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
// Per-tab chain of dismissal writes (see persistDismissal). Module scope:
// the create… factories run on every render, so a local map would be lost.
const dismissalQueue = new Map<string, Promise<void>>();

export function createProgress(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const { tabs, setTabs, activeTabId, addToast, tabsRef } = ctx;

  function reconcileViewedFiles(
    savedFiles: Record<string, string>,
    currentHashMap: Map<string, string>,
  ): { viewed: Set<string>; stale: Set<string> } {
    const viewed = new Set<string>();
    const stale = new Set<string>();
    for (const [path, savedHash] of Object.entries(savedFiles)) {
      const currentHash = currentHashMap.get(path);
      if (currentHash === undefined) continue;
      if (currentHash === savedHash) viewed.add(path); else stale.add(path);
    }
    return { viewed, stale };
  }

  async function loadPersistedViewedState(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<ViewedFileState | null>("load_viewed_files", { owner, repo, prNumber: number });
      if (!saved) return;

      const currentHashMap = new Map(tab.manifest.files.map((f) => [f.path, f.diff_hash]));
      const { viewed: viewedFiles, stale: staleViewedFiles } = reconcileViewedFiles(saved.files, currentHashMap);

      ctx.updateTab(tab.id, (t) => ({ ...t, viewedFiles, staleViewedFiles }));
    } catch {
      // Graceful degradation: if loading fails, start with empty state
    }
    syncGhViewedState(tab.id, tab.manifest.pr_url, tab.manifest.files);
  }

  async function loadDismissedHighlights(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<{ keys: string[]; resolutions?: Record<string, NoteResolution> } | null>("load_dismissed_highlights", { owner, repo, prNumber: number });
      if (saved && saved.keys.length > 0) {
        ctx.updateTab(tab.id, (t) => ({
          ...t,
          dismissedHighlights: new Set(saved.keys),
          noteResolutions: new Map(Object.entries(saved.resolutions ?? {})),
        }));
      }
    } catch {
      // Non-critical: start with nothing dismissed on failure
    }
  }

  /** Persist one dismissal change (issue #252). The backend applies it to
   * what's on disk key-by-key and returns the merged state, so a resolution
   * written meanwhile by another tool (the resolve script) is kept, never
   * overwritten, and shows up here. Writes for a tab run in order, so a slow
   * response can't roll back a newer one. */
  function persistDismissal(tabId: string, prUrl: string, command: "dismiss_highlight" | "restore_dismissed_highlight", args: Record<string, unknown>) {
    const { owner, repo, number } = parsePrUrl(prUrl);
    const prev = dismissalQueue.get(tabId) ?? Promise.resolve();
    const next = prev
      .then(() => invoke<{ keys: string[]; resolutions?: Record<string, NoteResolution> }>(command, { owner, repo, prNumber: number, ...args }))
      .then((saved) => {
        ctx.updateTab(tabId, (t) => ({
          ...t,
          dismissedHighlights: new Set(saved.keys),
          noteResolutions: new Map(Object.entries(saved.resolutions ?? {})),
        }));
      })
      .catch(() => addToast("error", "Couldn't save — this dismissal may not persist"));
    dismissalQueue.set(tabId, next);
  }

  /** Dismiss (hide) a note, optionally recording how/why it was resolved.
   * `resolution: null` is a plain/quick dismiss — no resolution metadata is
   * recorded (renders as the legacy "Dismissed" chip, same as noise). The
   * update is functional, so several in a row (a merged finding and its
   * duplicates) each apply to the latest state, not a stale snapshot. */
  function resolveHighlight(key: string, resolution: NoteResolution | null) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const stamped = resolution ? { ...resolution, at: new Date().toISOString() } : null;
    ctx.updateTab(tab.id, (t) => {
      const keys = new Set(t.dismissedHighlights);
      keys.add(key);
      const resolutions = new Map(t.noteResolutions);
      if (stamped) resolutions.set(key, stamped);
      else resolutions.delete(key);
      return { ...t, dismissedHighlights: keys, noteResolutions: resolutions };
    });
    persistDismissal(tab.id, tab.manifest.pr_url, "dismiss_highlight", { key, resolution: stamped });
  }

  /** Restore a previously-dismissed note, clearing any recorded resolution. */
  function restoreHighlight(key: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    ctx.updateTab(tab.id, (t) => {
      const keys = new Set(t.dismissedHighlights);
      keys.delete(key);
      const resolutions = new Map(t.noteResolutions);
      resolutions.delete(key);
      return { ...t, dismissedHighlights: keys, noteResolutions: resolutions };
    });
    persistDismissal(tab.id, tab.manifest.pr_url, "restore_dismissed_highlight", { key });
  }

  async function loadResolvedSpecs(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<{ keys: string[]; resolutions?: Record<string, NoteResolution> } | null>("load_resolved_specs", { owner, repo, prNumber: number });
      if (saved && saved.keys.length > 0) {
        ctx.updateTab(tab.id, (t) => ({
          ...t,
          resolvedSpecKeys: new Set(saved.keys),
          specResolutions: new Map(Object.entries(saved.resolutions ?? {})),
        }));
      }
    } catch {
      // Non-critical: start with nothing resolved on failure
    }
  }

  /** Persist the current tab's resolved-spec set + resolutions in one write. */
  function saveResolvedSpecs(tab: Tab, keys: Set<string>, resolutions: Map<string, NoteResolution>) {
    if (!tab.manifest) return;
    const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
    invoke("save_resolved_specs", {
      owner,
      repo,
      prNumber: number,
      state: { keys: [...keys], resolutions: Object.fromEntries(resolutions) },
    }).catch(() => addToast("error", "Couldn't save — this resolution may not persist"));
  }

  /** Mark a coverage-digest spec item (uncovered/partial requirement or orphan
   * test) addressed. This is a user acknowledgment, not a re-judgment of
   * coverage — the all-clear "requirements covered" fragment is untouched. */
  function resolveSpecItem(key: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const nextKeys = new Set(tab.resolvedSpecKeys);
    nextKeys.add(key);
    const nextResolutions = new Map(tab.specResolutions);
    nextResolutions.set(key, { state: "addressed", reason: "", at: new Date().toISOString() });
    ctx.updateTab(tab.id, (t) => ({ ...t, resolvedSpecKeys: nextKeys, specResolutions: nextResolutions }));
    saveResolvedSpecs(tab, nextKeys, nextResolutions);
  }

  /** Restore a previously-resolved spec item, clearing its recorded resolution. */
  function restoreSpecItem(key: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const nextKeys = new Set(tab.resolvedSpecKeys);
    nextKeys.delete(key);
    const nextResolutions = new Map(tab.specResolutions);
    nextResolutions.delete(key);
    ctx.updateTab(tab.id, (t) => ({ ...t, resolvedSpecKeys: nextKeys, specResolutions: nextResolutions }));
    saveResolvedSpecs(tab, nextKeys, nextResolutions);
  }

  async function loadCheckedFindings(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<{ entries: Record<string, CheckedFindingEntry> } | null>("load_checked_findings", { owner, repo, prNumber: number });
      if (saved && Object.keys(saved.entries).length > 0) {
        ctx.updateTab(tab.id, (t) => ({ ...t, checkedFindings: new Map(Object.entries(saved.entries)) }));
      }
    } catch {
      // Non-critical: start with nothing checked on failure
    }
  }

  function saveCheckedFindings(tab: Tab, entries: Map<string, CheckedFindingEntry>) {
    if (!tab.manifest) return;
    const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
    invoke("save_checked_findings", {
      owner,
      repo,
      prNumber: number,
      state: { entries: Object.fromEntries(entries) },
    }).catch(() => addToast("error", "Couldn't save — this mark may not persist"));
  }

  /** "Looks fine" on a finding (issue #238). Stores the hash of the code it
   * sits on, so the mark lapses — and the finding reopens — once a push
   * changes those lines (see buildFindings). */
  function markFindingChecked(finding: Pick<Finding, "key" | "linesHash">) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || !finding.linesHash) return;
    const next = new Map(tab.checkedFindings);
    next.set(finding.key, { lines_hash: finding.linesHash, at: new Date().toISOString() });
    ctx.updateTab(tab.id, (t) => ({ ...t, checkedFindings: next }));
    saveCheckedFindings(tab, next);
  }

  function unmarkFindingChecked(key: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || !tab.checkedFindings.has(key)) return;
    const next = new Map(tab.checkedFindings);
    next.delete(key);
    ctx.updateTab(tab.id, (t) => ({ ...t, checkedFindings: next }));
    saveCheckedFindings(tab, next);
  }

  async function loadLocalRequirements(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<{ text: string } | null>("load_pr_requirements", { owner, repo, prNumber: number });
      if (saved && saved.text) {
        ctx.updateTab(tab.id, (t) => ({ ...t, localRequirements: saved.text }));
      }
    } catch {
      // Non-critical: start with nothing saved on failure
    }
  }

  /** Persist the current tab's local requirements text (issue #179 phase 2).
   * The next analysis (on Refresh) judges tests against it — saving doesn't
   * auto-trigger a re-review. */
  function saveLocalRequirements(text: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    // Empty text means "back to PR-description extraction" — store null so
    // the UI treats the local source as absent (the backend gate already
    // ignores empty text). Persist the same normalized value the UI keeps,
    // so a reload can't diverge from the in-memory state.
    const normalized = text.trim();
    ctx.updateTab(tab.id, (t) => ({ ...t, localRequirements: normalized || null }));
    const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
    const prUrl = tab.manifest.pr_url;
    const tabId = tab.id;
    invoke("save_pr_requirements", {
      owner,
      repo,
      prNumber: number,
      state: { text: normalized },
    })
      .then(() => {
        // Saving is the ask — run the coverage pass right away instead of
        // waiting for a head change (Refresh cache-hits on unchanged heads).
        ctx.updateTab(tabId, (t) => ({ ...t, analyzingRequirements: true }));
        return invoke<ReviewManifest>("analyze_requirements", { prRef: prUrl }).then(
          (manifest) => {
            // Merge only the coverage section, and only onto the same head —
            // a concurrent Refresh may have replaced the manifest meanwhile,
            // and this analysis (built from the pre-Refresh cache) must not
            // clobber it.
            ctx.updateTab(tabId, (t) => ({
              ...t,
              manifest:
                t.manifest && t.manifest.head_sha === manifest.head_sha
                  ? { ...t.manifest, requirements_coverage: manifest.requirements_coverage }
                  : t.manifest ?? manifest,
              analyzingRequirements: false,
            }));
          },
          (e) => {
            ctx.updateTab(tabId, (t) => ({ ...t, analyzingRequirements: false }));
            addToast("error", `Couldn't analyze requirements: ${e}`);
          }
        );
      })
      .catch(() => addToast("error", "Couldn't save — these requirements may not persist"));
  }

  function toggleViewed(filePath: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const nowViewed = !tab.viewedFiles.has(filePath);
    const nextViewed = new Set(tab.viewedFiles);
    const nextStale = new Set(tab.staleViewedFiles);
    if (nowViewed) {
      nextViewed.add(filePath);
      nextStale.delete(filePath);
    } else {
      nextViewed.delete(filePath);
    }
    const updated = { ...tab, viewedFiles: nextViewed, staleViewedFiles: nextStale };
    ctx.updateTab(tab.id, () => updated);
    persistViewedState(updated);
    invoke("sync_file_viewed_to_github", { prUrl: tab.manifest.pr_url, path: filePath, viewed: nowViewed }).catch(() => {});
  }

  async function persistViewedState(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const fileHashMap = new Map(tab.manifest.files.map((f) => [f.path, f.diff_hash]));
      const files: Record<string, string> = {};
      for (const path of tab.viewedFiles) {
        const hash = fileHashMap.get(path);
        if (hash) files[path] = hash;
      }
      await invoke("save_viewed_files", { owner, repo, prNumber: number, state: { files } });
    } catch {
      // Non-critical: persistence failure shouldn't block UI
    }
  }

  async function syncGhViewedState(tabId: string, prUrl: string, files: Array<{ path: string; diff_hash: string }>) {
    try {
      const ghState = await invoke<Record<string, string>>("fetch_gh_viewed_state", { prUrl });
      const currentPaths = new Set(files.map((f) => f.path));

      setTabs((prev) => {
        const tab = prev.find((t) => t.id === tabId);
        if (!tab) return prev;

        const nextViewed = new Set(tab.viewedFiles);
        const nextStale = new Set(tab.staleViewedFiles);
        let changed = false;

        for (const [path, state] of Object.entries(ghState)) {
          if (!currentPaths.has(path)) continue;
          if (state === "VIEWED" && !nextViewed.has(path) && !nextStale.has(path)) {
            nextViewed.add(path);
            changed = true;
          } else if (state === "UNVIEWED" && nextViewed.has(path)) {
            nextViewed.delete(path);
            changed = true;
          } else if (state === "DISMISSED" && nextViewed.has(path)) {
            nextViewed.delete(path);
            nextStale.add(path);
            changed = true;
          }
        }

        if (!changed) return prev;
        const updated = { ...tab, viewedFiles: nextViewed, staleViewedFiles: nextStale };
        persistViewedState(updated);
        return prev.map((t) => (t.id === tabId ? updated : t));
      });
    } catch {
      // GH sync is best-effort — skip on failure (no token, network error, etc.)
    }
  }

  return {
    reconcileViewedFiles,
    loadPersistedViewedState,
    loadDismissedHighlights,

    resolveHighlight,
    restoreHighlight,
    loadResolvedSpecs,
    saveResolvedSpecs,
    resolveSpecItem,
    restoreSpecItem,
    loadCheckedFindings,
    saveCheckedFindings,
    markFindingChecked,
    unmarkFindingChecked,
    loadLocalRequirements,
    saveLocalRequirements,
    toggleViewed,
    persistViewedState,
    syncGhViewedState,
  };
}

export type ProgressApi = ReturnType<typeof createProgress>;

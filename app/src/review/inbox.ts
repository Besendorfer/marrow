// Inbox layout handlers (issue #238 phase 4): selection and the three finding
// actions. Everything funnels into existing handlers (handleChatOpenFile,
// setSelectedFile, resolveHighlight, markFindingChecked, …) so the inbox adds
// no new persistence paths of its own.

import type { FileDiff, NoteResolution } from "../types";
import { findingCommentBody, type Finding } from "./findings";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createInbox(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const { activeTabId, tabsRef, addToast, diffViewerRef, pendingComposerRef } = ctx;

  /** Show a finding: its file scrolled to its line when it has one (spec and
   * CI findings render their own panel instead). */
  function selectInboxFinding(f: Finding) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, inboxSelection: f.key, inboxSelectionPath: f.path ?? null }));
    if (f.path) ctx.handleChatOpenFile(f.path, f.startLine);
  }

  function selectInboxFile(file: FileDiff) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, inboxSelection: `file:${file.path}`, inboxSelectionPath: file.path }));
    ctx.setSelectedFile(file);
  }

  /** Non-file panels (About, Spec, CI) — just the selection. */
  function selectInboxPanel(key: string) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, inboxSelection: key, inboxSelectionPath: null }));
  }

  /** Spec findings act on their requirements (the per-requirement store the
   * RequirementsCard uses), in ONE write — resolveSpecItem reads a tab
   * snapshot, so calling it per key in a loop would keep only the last. */
  function addressSpecItems(keys: string[]) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || keys.length === 0) return;
    const nextKeys = new Set(tab.resolvedSpecKeys);
    const nextResolutions = new Map(tab.specResolutions);
    const at = new Date().toISOString();
    for (const key of keys) {
      nextKeys.add(key);
      nextResolutions.set(key, { state: "addressed", reason: "", at });
    }
    ctx.updateTab(tab.id, (t) => ({ ...t, resolvedSpecKeys: nextKeys, specResolutions: nextResolutions }));
    ctx.saveResolvedSpecs(tab, nextKeys, nextResolutions);
  }

  function inboxLooksFine(f: Finding) {
    if (f.kind === "spec") addressSpecItems(f.itemKeys ?? []);
    else ctx.markFindingChecked(f);
  }

  /** "Not an issue": dismiss with an optional how/why (null = plain). Notes
   * hide from the diff like any dismissal; risk/CI keys are inert there. */
  function inboxNotAnIssue(f: Finding, resolution: NoteResolution | null) {
    if (f.kind === "spec") addressSpecItems(f.itemKeys ?? []);
    else ctx.resolveHighlight(f.key, resolution);
  }

  /** Undo a Looks fine / Not an issue. */
  function inboxReopen(f: Finding) {
    if (f.state === "dismissed") ctx.restoreHighlight(f.key);
    else if (f.state === "checked") ctx.unmarkFindingChecked(f.key);
  }

  /** "Comment": open the inline composer on the finding's lines, prefilled.
   * Nothing posts until the reviewer submits it (it joins the pending review).
   * Findings without a line (spec, CI) draft a PR-level comment instead. */
  function inboxComment(f: Finding) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    const body = findingCommentBody(f);
    if (f.path == null || f.startLine == null) {
      ctx.updateTab(tab.id, (t) => ({
        ...t,
        commentsOpen: true,
        chat: { ...t.chat, open: false },
        prCommentDraft: body,
      }));
      return;
    }
    const composer = { startLine: f.startLine, endLine: f.endLine ?? f.startLine, side: "RIGHT" as const, initialBody: body };
    if (tab.selectedFile?.path === f.path && tab.lens === "files") {
      if (!diffViewerRef.current?.openComposer(composer.startLine, composer.endLine, composer.side, composer.initialBody)) {
        addToast("info", `Line ${f.startLine} isn't in this file's current diff — comment from the diff instead.`);
      }
      return;
    }
    const file = tab.manifest.files.find((x) => x.path === f.path);
    if (!file) return;
    // Same deferral the chat draft_comment chip uses: open once the viewer mounts.
    pendingComposerRef.current = composer;
    ctx.setSelectedFile(file);
  }

  return {
    selectInboxFinding,
    selectInboxFile,
    selectInboxPanel,
    addressSpecItems,
    inboxLooksFine,
    inboxNotAnIssue,
    inboxReopen,
    inboxComment,
  };
}

export type InboxApi = ReturnType<typeof createInbox>;

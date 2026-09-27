// Inbox layout handlers (issue #238 phase 4): selection and the three finding
// actions. Everything funnels into existing handlers (handleChatOpenFile,
// setSelectedFile, resolveHighlight, markFindingChecked, …) so the inbox adds
// no new persistence paths of its own.

import type { FileDiff, NoteResolution } from "../types";
import { findingCommentBody, selectionIdFor, type Finding } from "./findings";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createInbox(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const { activeTabId, tabsRef, addToast, diffViewerRef, pendingComposerRef, pendingRevealLineRef } = ctx;

  /** Select `selectionId` and show `path` at `line`. When the file is
   * already the open one, the diff may still be unmounted (a panel — About,
   * Spec, CI — was showing), so the reveal is queued for the pending-reveal
   * effect, which reruns on selection changes, instead of calling into a
   * viewer that may not exist yet. */
  function openInInbox(selectionId: string, path: string, line?: number) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    ctx.updateTab(tab.id, (t) => ({ ...t, inboxSelection: selectionId, inboxSelectionPath: path }));
    const open = tab.selectedFile;
    // Head contents missing means handleChatOpenFile must fetch them before a
    // line in unchanged code can be shown — let it take that path.
    const canReveal = line == null || !!open?.head_content || open?.diff_type === "removed";
    if (open?.path === path && canReveal) {
      pendingRevealLineRef.current = line ?? null;
      if (tab.lens !== "files") ctx.updateTab(tab.id, (t) => ({ ...t, lens: "files" }));
      return;
    }
    ctx.handleChatOpenFile(path, line);
  }

  /** Show a finding: its file scrolled to its line when it has one (spec and
   * CI findings render their own panel instead). */
  function selectInboxFinding(f: Finding) {
    if (f.path) openInInbox(selectionIdFor(f), f.path, f.startLine);
    else selectInboxPanel(selectionIdFor(f));
  }

  /** A location link inside an inbox panel (About, Spec, CI annotations):
   * move the selection to that file, then reveal the line. */
  function inboxOpenAt(path: string, line?: number) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    const target = ctx.resolveManifestFile(tab.manifest.files, path);
    if (!target) return;
    openInInbox(`file:${target.path}`, target.path, line);
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

  /** Returns whether a mark was made (the list advances only then). */
  function inboxLooksFine(f: Finding): boolean {
    if (f.kind === "spec") {
      addressSpecItems(f.itemKeys ?? []);
      return true;
    }
    if (!f.linesHash) {
      // A risk on a file outside the diff has no code to anchor a mark to.
      addToast("info", "There's no code here to anchor “Looks fine” to — use Not an issue instead.");
      return false;
    }
    ctx.markFindingChecked(f);
    return true;
  }

  /** "Not an issue": dismiss with an optional how/why (null = plain). Notes
   * hide from the diff like any dismissal; risk/CI keys are inert there. */
  function inboxNotAnIssue(f: Finding, resolution: NoteResolution | null) {
    if (f.kind === "spec") return addressSpecItems(f.itemKeys ?? []);
    // Changing your mind from Looks fine: one verdict per finding, so the
    // earlier mark goes (else Reopen would surface it as a second state).
    ctx.unmarkFindingChecked(f.key);
    ctx.resolveHighlight(f.key, resolution);
  }

  /** Undo a Looks fine / Not an issue — both stores, so nothing stale resurfaces. */
  function inboxReopen(f: Finding) {
    ctx.unmarkFindingChecked(f.key);
    if (f.state === "dismissed") ctx.restoreHighlight(f.key);
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
    inboxOpenAt,
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

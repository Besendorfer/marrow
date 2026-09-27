// Chat handlers (moved verbatim from App.tsx in issue #238 phase 2).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke, Channel } from "@tauri-apps/api/core";
import type { FileDiff, Tab, ChatMessage, ChatStreamEvent, ChatAction } from "../types";
import { parsePrUrl } from "../utils";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createChat(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const {
    activeTabId,
    setViewMode,
    setHunkFilter,
    diffViewerRef,
    setChatActionStatuses,
    chatExecutedActionsRef,
    activeTab,
    chatCancelRef,
    chatRequestIdRef,
    pendingComposerRef,
    briefMePendingRef,
    setBriefMePing,
    tabsRef,
  } = ctx;

  async function loadChatHistory(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<{ messages: ChatMessage[] } | null>("load_chat_history", { owner, repo, prNumber: number });
      if (saved && saved.messages.length > 0) {
        ctx.updateTab(tab.id, (t) => ({ ...t, chat: { ...t.chat, messages: saved.messages } }));
      }
    } catch {
      // Non-critical: start with an empty conversation on failure
    }
  }

  /** The diff/content context for the chat. Effective scope is the whole PR
   * (relevant files only) when `includeWholePr` is set OR no file is selected
   * (the overview) — auto-scope means there's no "select a file first" error
   * path. Whole-PR omits full contents to save budget. AI highlights ride
   * along so questions about "the warning on L287-318" resolve against them. */
  function buildChatFiles(tab: Tab): Array<{ path: string; unified_diff: string; head_content?: string; highlights: FileDiff["highlights"] }> {
    const manifest = tab.manifest!;
    if (tab.chat.includeWholePr || !tab.selectedFile) {
      const relevant = manifest.files.filter((f) => f.classification === "RELEVANT");
      const files = relevant.length > 0 ? relevant : manifest.files;
      return files.map((f) => ({ path: f.path, unified_diff: f.unified_diff, highlights: f.highlights }));
    }
    const f = tab.selectedFile;
    return [{ path: f.path, unified_diff: f.unified_diff, head_content: f.head_content, highlights: f.highlights }];
  }

  /** Append the assistant's answer, return the chat to idle, and persist. */
  function finalizeChat(tabId: string, prUrl: string, content: string) {
    const tab = tabsRef.current.find((t) => t.id === tabId);
    const messages: ChatMessage[] = [...(tab?.chat.messages ?? []), { role: "assistant", content }];
    ctx.updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, messages, status: "idle", streamingText: "", streamingStatus: null } }));
    // The just-finished turn's action statuses were recorded under the
    // "streaming" bucket — move them to this message's own key (its index in
    // the now-final array) so its chips keep showing ✓/✗ after finalize.
    const msgKey = `msg-${messages.length - 1}`;
    setChatActionStatuses((prev) => {
      const streaming = prev[tabId]?.streaming;
      if (!streaming) return prev;
      const nextForTab = { ...prev[tabId] };
      delete nextForTab.streaming;
      nextForTab[msgKey] = streaming;
      return { ...prev, [tabId]: nextForTab };
    });
    try {
      const { owner, repo, number } = parsePrUrl(prUrl);
      invoke("save_chat_history", { owner, repo, prNumber: number, state: { messages } }).catch(() => {});
    } catch {
      // Non-critical: an unparseable URL just means this turn isn't persisted.
    }
  }

  function handleChatSend(message: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const tabId = tab.id;
    const manifest = tab.manifest;
    const files = buildChatFiles(tab);
    if (files.length === 0) return;
    const userMsg: ChatMessage = {
      role: "user",
      content: message,
      filePath: tab.chat.includeWholePr ? undefined : tab.selectedFile?.path,
    };
    // Cap the history sent to the model — full history still renders and persists.
    const history = tab.chat.messages.slice(-12).map((m) => ({ role: m.role, content: m.content }));
    const requestId = crypto.randomUUID();

    chatCancelRef.current[tabId] = false;
    chatRequestIdRef.current[tabId] = requestId;
    // A fresh turn starts a fresh action-block execution window.
    chatExecutedActionsRef.current[tabId] = new Set();
    setChatActionStatuses((prev) => ({ ...prev, [tabId]: { ...prev[tabId], streaming: {} } }));
    ctx.updateTab(tabId, (t) => ({
      ...t,
      chat: { ...t.chat, messages: [...t.chat.messages, userMsg], status: "streaming", streamingText: "", streamingStatus: null, error: undefined },
    }));

    const channel = new Channel<ChatStreamEvent>();
    channel.onmessage = (ev) => {
      // Drop events from a cancelled request AND from a superseded one: after
      // Stop → new send, stragglers from the old stream can still arrive
      // (chat_cancel is fire-and-forget) and must not touch the new request.
      if (chatCancelRef.current[tabId] || chatRequestIdRef.current[tabId] !== requestId) return;
      if (ev.type === "delta") {
        // Any text clears a pending "Working…" status.
        ctx.updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, streamingText: t.chat.streamingText + ev.text, streamingStatus: null } }));
      } else if (ev.type === "status") {
        ctx.updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, streamingStatus: ev.label } }));
      } else if (ev.type === "done") {
        finalizeChat(tabId, manifest.pr_url, ev.content);
      } else if (ev.type === "error") {
        ctx.updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, status: "idle", streamingText: "", streamingStatus: null, error: ev.message } }));
      }
    };

    invoke("chat_send", {
      channel,
      request: {
        request_id: requestId,
        context: { pr_title: manifest.pr_title, summary: manifest.summary, files },
        history,
        message,
        // Repo identity for the read-only repo tools (issue #150); absent
        // (rather than throwing) falls back to diff-only chat server-side.
        repo: (() => {
          try {
            const { owner, repo } = parsePrUrl(manifest.pr_url);
            return { owner, repo, head_sha: manifest.head_sha };
          } catch {
            return undefined;
          }
        })(),
      },
    }).catch((err) => {
      if (chatCancelRef.current[tabId] || chatRequestIdRef.current[tabId] !== requestId) return;
      ctx.updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, status: "idle", streamingText: "", error: String(err) } }));
    });
  }

  /** Abort the in-flight stream (server-side too) and keep whatever streamed so far. */
  function handleChatStop() {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    chatCancelRef.current[tab.id] = true;
    const requestId = chatRequestIdRef.current[tab.id];
    if (requestId) invoke("chat_cancel", { requestId }).catch(() => {});
    const partial = tab.chat.streamingText.trim();
    if (partial) {
      finalizeChat(tab.id, tab.manifest.pr_url, partial);
    } else {
      ctx.updateTab(tab.id, (t) => ({ ...t, chat: { ...t.chat, status: "idle", streamingText: "" } }));
    }
  }

  function handleChatClear() {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    chatCancelRef.current[tab.id] = true;
    if (tab.chat.status === "streaming") {
      const requestId = chatRequestIdRef.current[tab.id];
      if (requestId) invoke("chat_cancel", { requestId }).catch(() => {});
    }
    delete chatExecutedActionsRef.current[tab.id];
    setChatActionStatuses((prev) => {
      const copy = { ...prev };
      delete copy[tab.id];
      return copy;
    });
    ctx.updateTab(tab.id, (t) => ({ ...t, chat: { ...t.chat, messages: [], status: "idle", streamingText: "", error: undefined } }));
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      invoke("save_chat_history", { owner, repo, prNumber: number, state: { messages: [] } }).catch(() => {});
    } catch {
      // Non-critical.
    }
  }

  function setChatOpen(open: boolean) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, chat: { ...t.chat, open } }));
  }

  // Chat and Comments are mutually exclusive right-dock panels — opening chat closes comments.
  function withChatOpen(t: Tab, open: boolean): Tab {
    return { ...t, chat: { ...t.chat, open }, commentsOpen: open ? false : t.commentsOpen };
  }

  function toggleChatOpen() {
    ctx.updateTab(activeTabId, (t) => withChatOpen(t, !t.chat.open));
  }

  function handleChatToggleWholePr(value: boolean) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, chat: { ...t.chat, includeWholePr: value } }));
  }

  // ---- Chat ```marrow-action view-control blocks (issue #166) ----

  /** Run one chat-emitted view-control action against the active tab. No
   * mutating actions here — everything is navigation/view state, with one
   * carve-out: draft_comment, which is manual-only (never auto-executed; the
   * user must click its chip) and still posts nothing — it only opens the
   * local comment composer prefilled, and posting stays behind the composer's
   * own submit. Returns whether the action resolved (e.g. the file/commit it
   * names actually exists) so the caller can render a done/failed chip. */
  function executeChatAction(a: ChatAction): boolean {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return false;
    switch (a.action) {
      case "open_file": {
        if (!ctx.resolveManifestFile(tab.manifest.files, a.path)) return false;
        ctx.handleChatOpenFile(a.path, a.line);
        return true;
      }
      case "open_overview":
        // selectedFile is left as-is (Files lens returns to where it was).
        ctx.setLens(tab.id, "overview");
        return true;
      case "next_file":
        return ctx.selectAdjacentFile(1);
      case "prev_file":
        return ctx.selectAdjacentFile(-1);
      case "open_commit": {
        const commits = tab.manifest.commits;
        const exact = commits.find((c) => c.sha === a.sha);
        // A prefix must be unambiguous — resolving to "whichever came first"
        // could open the wrong commit while reporting success.
        const prefixed = commits.filter((c) => c.sha.startsWith(a.sha));
        const match = exact ?? (prefixed.length === 1 ? prefixed[0] : undefined);
        if (!match) return false;
        ctx.handleViewCommit(match, tab.id);
        return true;
      }
      case "set_hunk_filter":
        setHunkFilter(a.filter);
        return true;
      case "set_view_mode":
        setViewMode(a.mode);
        return true;
      case "show_comments":
        // Chat and Comments are mutually exclusive right-dock panels (see
        // toggleThreadsView) — opening comments from here closes chat too.
        ctx.updateTab(tab.id, (t) => ({
          ...t,
          commentsOpen: a.open,
          chat: a.open ? { ...t.chat, open: false } : t.chat,
        }));
        return true;
      case "draft_comment": {
        const target = ctx.resolveManifestFile(tab.manifest.files, a.path);
        if (!target) return false;
        const composer = {
          startLine: a.start_line ?? a.line,
          endLine: a.line,
          side: "RIGHT" as const,
          initialBody: a.body,
        };
        if (target.path === tab.selectedFile?.path && tab.lens === "files") {
          // Already viewing the file — the mounted DiffViewer can open directly.
          return diffViewerRef.current?.openComposer(composer.startLine, composer.endLine, composer.side, composer.initialBody) ?? false;
        } else if (target.path === tab.selectedFile?.path) {
          // Right file, wrong lens: switch to Files and let the pending-composer
          // effect open once the viewer mounts (same deferral as reveals).
          pendingComposerRef.current = composer;
          ctx.updateTab(tab.id, (t) => ({ ...t, lens: "files" }));
        } else {
          pendingComposerRef.current = composer;
          ctx.setSelectedFile(target);
        }
        return true;
      }
      case "draft_pr_comment":
        // Manual-only chip (see the auto-exec skip below): open the Comments
        // panel with the PR-level compose box prefilled — same mutual
        // exclusion with chat as show_comments.
        ctx.updateTab(tab.id, (t) => ({
          ...t,
          commentsOpen: true,
          chat: { ...t.chat, open: false },
          prCommentDraft: a.body,
        }));
        return true;
      default:
        return false;
    }
  }

  /** Execute a chat action chip's click (or the streaming auto-exec effect
   * below), and record its done/failed status under the message it belongs
   * to. `msgKey` is "msg-<index>" for a finalized message or "streaming" for
   * the in-progress turn; `blockIndex` is the action's position among that
   * message's closed marrow-action fences (see RichText's parseActionFences). */
  function runChatAction(tabId: string, msgKey: string, a: ChatAction, blockIndex: number) {
    const ok = executeChatAction(a);
    const key = `${blockIndex}:${JSON.stringify(a)}`;
    setChatActionStatuses((prev) => ({
      ...prev,
      [tabId]: {
        ...prev[tabId],
        [msgKey]: { ...prev[tabId]?.[msgKey], [key]: ok ? "done" : "failed" },
      },
    }));
  }

  /** AI walkthrough of the whole PR, most-important-first — opens/expands
   * chat to whole-PR scope and asks it to narrate the changes. */
  function briefMe() {
    if (!activeTab?.manifest) return;
    ctx.updateTab(activeTabId, (t) => withChatOpen(t, true));
    handleChatToggleWholePr(true);
    if (activeTab.chat.status === "streaming") return;
    briefMePendingRef.current = true;
    setBriefMePing((p) => p + 1);
  }

  return {
    loadChatHistory,
    buildChatFiles,
    finalizeChat,
    handleChatSend,
    handleChatStop,
    handleChatClear,
    setChatOpen,
    withChatOpen,
    toggleChatOpen,
    handleChatToggleWholePr,
    executeChatAction,
    runChatAction,
    briefMe,
  };
}

export type ChatApi = ReturnType<typeof createChat>;

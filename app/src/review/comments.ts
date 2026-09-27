// Comments handlers (moved verbatim from App.tsx in issue #238 phase 2).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke } from "@tauri-apps/api/core";
import type { ReviewThread, ReviewComment, PrConversationComment, MyReviewState } from "../types";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createComments(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const {
    tabs,
    setTabs,
    activeTabId,
    setError,
    pendingThreadIdRef,
    setThreadScrollPing,
    addToast,
    activeTab,
    tabsRef,
  } = ctx;

  // Chat and Comments are mutually exclusive right-dock panels — opening one closes the other.
  function toggleThreadsView() {
    if (!activeTab?.manifest) return;
    const opening = !activeTab.commentsOpen;
    ctx.updateTab(activeTabId, (t) => ({
      ...t,
      commentsOpen: opening,
      chat: opening ? { ...t.chat, open: false } : t.chat,
    }));
    // Fetch is driven by the commentsOpen+idle effect below, so every path
    // that opens the panel (toggle, palette, legacy session restore) fetches.
  }

  async function fetchMyReviewState(tabId: string, prUrl: string) {
    try {
      const state = await invoke<MyReviewState>("get_my_review_state", { prUrl });
      ctx.updateTab(tabId, (t) => ({ ...t, myReviewState: state }));
    } catch {
      // Non-critical: if fetching fails, button stays enabled
    }
  }

  function setCommentsOpen(open: boolean) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, commentsOpen: open }));
  }

  async function handleRefreshComments(tabId: string) {
    const tab = tabsRef.current.find((t) => t.id === tabId);
    if (!tab || !tab.manifest || tab.isRefreshing) return;

    refreshPrConversation(tabId, tab.manifest.pr_url);
    try {
      const threads = await invoke<ReviewThread[]>("fetch_review_comments", {
        prUrl: tab.manifest.pr_url,
      });

      const oldCount =
        tab.commentThreads.status === "loaded"
          ? tab.commentThreads.threads.reduce((n, t) => n + t.comments.length, 0)
          : 0;
      const newCount = threads.reduce((n, t) => n + t.comments.length, 0);
      const diff = newCount - oldCount;

      ctx.updateTab(tabId, (t) => ({
        ...t,
        commentThreads: { status: "loaded", threads },
        lastCommentCount: newCount,
      }));

      if (diff > 0) {
        addToast("info", `${diff} new comment${diff > 1 ? "s" : ""} on PR #${tab.manifest.pr_number}`);
      }
    } catch {
      // Poll will retry
    }
  }

  /** Best-effort fetch of the PR-level conversation comments (issue #185).
   * Deliberately fire-and-forget from the thread-fetch paths: a failure keeps
   * the prior value and must never block or fail the threads fetch. */
  async function refreshPrConversation(tabId: string, prUrl: string) {
    try {
      const comments = await invoke<PrConversationComment[]>("fetch_pr_conversation", { prUrl });
      ctx.updateTab(tabId, (t) => ({ ...t, prConversation: comments }));
    } catch {
      // Keep whatever we had (possibly null = never loaded).
    }
  }

  /** Post a top-level PR conversation comment, then refresh the conversation
   * list (only — threads are untouched by a conversation comment). Returns
   * whether the post succeeded so the compose box knows to keep its text. */
  async function handlePostPrComment(body: string): Promise<boolean> {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return false;
    try {
      await invoke<string>("add_pr_comment", { prUrl: tab.manifest.pr_url, body });
      ctx.updateTab(tab.id, (t) => ({ ...t, prCommentDraft: null }));
      refreshPrConversation(tab.id, tab.manifest.pr_url);
      addToast("success", "Comment posted");
      return true;
    } catch (err) {
      addToast("error", `Failed to post comment: ${String(err)}`);
      return false;
    }
  }

  async function handleRequestComments() {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || tab.commentThreads.status === "loading" || tab.commentThreads.status === "loaded") return;

    ctx.updateTab(activeTabId,(t) => ({ ...t, commentThreads: { status: "loading" } }));
    refreshPrConversation(tab.id, tab.manifest.pr_url);
    try {
      const threads = await invoke<ReviewThread[]>("fetch_review_comments", {
        prUrl: tab.manifest.pr_url,
      });
      ctx.updateTab(activeTabId,(t) => ({ ...t, commentThreads: { status: "loaded", threads } }));
    } catch (err) {
      ctx.updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "error", message: String(err) },
      }));
    }
  }

  /** File-group header click in the comments panel — jump to the file, no thread scroll. */
  function handleOpenCommentFile(path: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    const file = tab.manifest.files.find((f) => f.path === path);
    if (file) ctx.setSelectedFile(file);
    else addToast("info", "File not in this diff");
  }

  /** Thread-card location click in the comments panel — select the file (if
   * needed) then scroll/flash the thread into view once the diff has mounted it. */
  function handleJumpToThread(thread: ReviewThread) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    if (tab.selectedFile?.path !== thread.path) {
      const file = tab.manifest.files.find((f) => f.path === thread.path);
      if (!file) {
        addToast("info", "File not in this diff");
        return;
      }
      ctx.setSelectedFile(file);
    }
    pendingThreadIdRef.current = thread.id;
    setThreadScrollPing((p) => p + 1);
  }

  async function handleReply(threadId: string, commentId: string, body: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || tab.commentThreads.status !== "loaded") return;

    // Optimistic update: add a placeholder comment
    const optimisticComment = {
      id: `optimistic-${Date.now()}`,
      body,
      author: { login: "you", avatar_url: "" },
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      url: "",
      reactions: [],
    };

    const prevThreads = tab.commentThreads.threads;
    const optimisticThreads = prevThreads.map((t) =>
      t.id === threadId
        ? { ...t, comments: [...t.comments, optimisticComment] }
        : t
    );
    ctx.updateTab(activeTabId,(t) => ({
      ...t,
      commentThreads: { status: "loaded", threads: optimisticThreads },
    }));

    try {
      const newComment = await invoke<ReviewComment>("reply_to_thread", {
        prUrl: tab.manifest.pr_url,
        commentId,
        body,
      });

      // Replace optimistic comment with real one
      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== activeTabId || t.commentThreads.status !== "loaded") return t;
          return {
            ...t,
            commentThreads: {
              status: "loaded",
              threads: t.commentThreads.threads.map((th) =>
                th.id === threadId
                  ? {
                      ...th,
                      comments: th.comments.map((c) =>
                        c.id === optimisticComment.id ? newComment : c
                      ),
                    }
                  : th
              ),
            },
          };
        })
      );
    } catch {
      // Revert on error
      ctx.updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "loaded", threads: prevThreads },
      }));
    }
  }

  async function handleToggleResolved(threadId: string, resolve: boolean) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || tab.commentThreads.status !== "loaded") return;

    const prevThreads = tab.commentThreads.threads;

    // Optimistic update
    const optimisticThreads = prevThreads.map((t) =>
      t.id === threadId ? { ...t, is_resolved: resolve } : t
    );
    ctx.updateTab(activeTabId,(t) => ({
      ...t,
      commentThreads: { status: "loaded", threads: optimisticThreads },
    }));

    try {
      await invoke<boolean>("toggle_thread_resolved", { threadId, resolve });
    } catch (err) {
      // Revert on error — and say so, or the button just looks dead (#72)
      ctx.updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "loaded", threads: prevThreads },
      }));
      addToast("error", `Couldn't ${resolve ? "resolve" : "unresolve"} the thread: ${String(err)}`);
    }
  }

  async function handleEditComment(commentId: string, body: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || tab.commentThreads.status !== "loaded") return;

    const prevThreads = tab.commentThreads.threads;

    // Optimistic update
    const optimisticThreads = prevThreads.map((t) => ({
      ...t,
      comments: t.comments.map((c) =>
        c.id === commentId ? { ...c, body } : c
      ),
    }));
    ctx.updateTab(activeTabId,(t) => ({
      ...t,
      commentThreads: { status: "loaded" as const, threads: optimisticThreads },
    }));

    try {
      const updated = await invoke<ReviewComment>("update_review_comment", {
        commentId,
        body,
      });

      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== activeTabId || t.commentThreads.status !== "loaded") return t;
          return {
            ...t,
            commentThreads: {
              status: "loaded" as const,
              threads: t.commentThreads.threads.map((th) => ({
                ...th,
                comments: th.comments.map((c) =>
                  c.id === commentId ? updated : c
                ),
              })),
            },
          };
        })
      );
    } catch {
      ctx.updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "loaded" as const, threads: prevThreads },
      }));
    }
  }

  async function handleToggleReaction(commentId: string, content: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || tab.commentThreads.status !== "loaded") return;

    const prevThreads = tab.commentThreads.threads;

    // Single pass: determine add vs remove and build optimistic update together
    let willAdd = true;
    const optimisticThreads = prevThreads.map((th) => {
      if (!th.comments.some((c) => c.id === commentId)) return th;
      return {
        ...th,
        comments: th.comments.map((c) => {
          if (c.id !== commentId) return c;
          const reactions = c.reactions ?? [];
          const existing = reactions.find((r) => r.content === content);
          const add = !(existing?.viewer_has_reacted);
          willAdd = add;
          if (add) {
            if (existing) {
              return { ...c, reactions: reactions.map((r) => r.content === content ? { ...r, total_count: r.total_count + 1, viewer_has_reacted: true } : r) };
            }
            return { ...c, reactions: [...reactions, { content, total_count: 1, viewer_has_reacted: true }] };
          }
          return {
            ...c,
            reactions: reactions
              .map((r) => r.content === content ? { ...r, total_count: r.total_count - 1, viewer_has_reacted: false } : r)
              .filter((r) => r.total_count > 0),
          };
        }),
      };
    });
    ctx.updateTab(activeTabId, (t) => ({
      ...t,
      commentThreads: { status: "loaded" as const, threads: optimisticThreads },
    }));

    try {
      await invoke("toggle_reaction", { commentId, content, add: willAdd });
    } catch {
      ctx.updateTab(activeTabId, (t) => ({
        ...t,
        commentThreads: { status: "loaded" as const, threads: prevThreads },
      }));
    }
  }

  /** Submit the review. Throws on failure so the Finish panel can show the
   * error in place (issue #238) — it used to replace the whole PR screen. */
  async function handleSubmitReview(event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;

    await invoke<string>("submit_review", {
      prUrl: tab.manifest.pr_url,
      event,
      body,
    });

    {
      // Update review state immediately (submitting clears the review request)
      const statusMap: Record<string, string> = {
        APPROVE: "approved",
        REQUEST_CHANGES: "changes_requested",
        COMMENT: "commented",
      };
      ctx.updateTab(activeTabId, (t) => ({
        ...t,
        myReviewState: {
          author: t.myReviewState?.author ?? "",
          draft: t.myReviewState?.draft ?? false,
          approved_by: t.myReviewState?.approved_by ?? [],
          status: statusMap[event] as MyReviewState["status"],
          is_re_requested: false,
          is_merged: t.myReviewState?.is_merged ?? false,
          mergeable: t.myReviewState?.mergeable ?? "",
          labels: t.myReviewState?.labels ?? [],
          last_reviewed_sha: t.myReviewState?.last_reviewed_sha ?? null,
          last_reviewed_at: t.myReviewState?.last_reviewed_at ?? null,
        },
      }));

      // Re-fetch threads: submitting publishes pending comments and can
      // change resolved states. Best-effort — the review itself is in.
      try {
        const threads = await invoke<ReviewThread[]>("fetch_review_comments", {
          prUrl: tab.manifest.pr_url,
        });
        ctx.updateTab(activeTabId,(t) => ({
          ...t,
          commentThreads: { status: "loaded" as const, threads },
        }));
      } catch {
        // Stale thread state is fine; the panel shows the submitted review.
      }
    }
  }

  async function handleCreateComment(path: string, endLine: number, side: "LEFT" | "RIGHT", body: string, startLine?: number, startSide?: "LEFT" | "RIGHT") {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;

    try {
      const newThread = await invoke<ReviewThread>("create_review_comment", {
        prUrl: tab.manifest.pr_url,
        body,
        path,
        line: endLine,
        side,
        startLine: startLine ?? null,
        startSide: startSide ?? null,
      });

      // Add the new thread to the comment threads state
      ctx.updateTab(activeTabId,(t) => {
        if (t.commentThreads.status === "loaded") {
          return {
            ...t,
            commentThreads: {
              status: "loaded",
              threads: [...t.commentThreads.threads, newThread],
            },
          };
        }
        return {
          ...t,
          commentThreads: { status: "loaded", threads: [newThread] },
        };
      });
    } catch (err) {
      setError(String(err));
    }
  }

  return {
    toggleThreadsView,
    fetchMyReviewState,
    setCommentsOpen,
    handleRefreshComments,
    refreshPrConversation,
    handlePostPrComment,
    handleRequestComments,
    handleOpenCommentFile,
    handleJumpToThread,
    handleReply,
    handleToggleResolved,
    handleEditComment,
    handleToggleReaction,
    handleSubmitReview,
    handleCreateComment,
  };
}

export type CommentsApi = ReturnType<typeof createComments>;

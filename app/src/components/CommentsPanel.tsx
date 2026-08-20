import { useEffect, useMemo, useRef, useState } from "react";
import type { CommentThreadsState, PrConversationComment, ReviewComment, ReviewThread } from "../types";
import { getFileName, timeAgo } from "../utils";
import { CommentBodyRendered, ReactionBar, detectLanguage } from "./DiffViewer";

type CommentsFilter = "unresolved" | "all";

interface CommentsPanelProps {
  commentThreads: CommentThreadsState;
  /** Top-level PR conversation comments (issue #185). null = not loaded yet
   * (fetched best-effort alongside the threads) — the list area stays empty. */
  conversation: PrConversationComment[] | null;
  /** Pending draft from the chat draft_pr_comment chip — seeds the compose
   * box (which remounts per new draft). null = start empty. */
  composeInitialBody: string | null;
  /** Post a PR-level conversation comment; resolves true on success. */
  onPostPrComment: (body: string) => Promise<boolean>;
  /** Discard the pending compose draft (after a successful post or Cancel). */
  onClearComposeDraft: () => void;
  onRetry: () => void;
  onReply: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved: (threadId: string, resolve: boolean) => void;
  onEditComment?: (commentId: string, body: string) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  onClose: () => void;
  /** Jump straight to a file (from a file-group header) — no thread scroll. */
  onOpenFile: (path: string) => void;
  /** Select the thread's file (if needed) and scroll/flash it into view. */
  onJumpToThread: (thread: ReviewThread) => void;
}

function CommentCard({
  comment,
  onEdit,
  onToggleReaction,
  lang,
}: {
  comment: ReviewComment;
  onEdit?: (commentId: string, body: string) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  lang?: string;
}) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(comment.body);

  function handleSave() {
    if (!text.trim() || !onEdit) return;
    onEdit(comment.id, text.trim());
    setEditing(false);
  }

  return (
    <div className="thread-comment">
      <div className="comment-header">
        {comment.author.avatar_url && (
          <img
            className="comment-avatar"
            src={comment.author.avatar_url}
            alt={comment.author.login}
            width={20}
            height={20}
          />
        )}
        <span className="comment-author">@{comment.author.login}</span>
        <span className="comment-time">{timeAgo(comment.created_at)}</span>
        {onEdit && !editing && (
          <button className="comment-edit-button" onClick={() => setEditing(true)}>Edit</button>
        )}
      </div>
      {editing ? (
        <div className="comment-edit-form">
          <textarea
            className="reply-textarea"
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={Math.max(3, text.split("\n").length + 1)}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) handleSave();
              if (e.key === "Escape") { setText(comment.body); setEditing(false); }
            }}
          />
          <div className="reply-actions">
            <button className="reply-cancel" onClick={() => { setText(comment.body); setEditing(false); }}>Cancel</button>
            <button className="reply-submit" disabled={!text.trim()} onClick={handleSave}>Save</button>
          </div>
        </div>
      ) : (
        <>
          <CommentBodyRendered body={comment.body} lang={lang} />
          {onToggleReaction && <ReactionBar reactions={comment.reactions ?? []} commentId={comment.id} onToggleReaction={onToggleReaction} />}
        </>
      )}
    </div>
  );
}

function ThreadCard({
  thread,
  onReply,
  onToggleResolved,
  onEdit,
  onToggleReaction,
  onJumpToThread,
  lang,
}: {
  thread: ReviewThread;
  onReply: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved: (threadId: string, resolve: boolean) => void;
  onEdit?: (commentId: string, body: string) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  onJumpToThread: (thread: ReviewThread) => void;
  lang?: string;
}) {
  const [threadCollapsed, setThreadCollapsed] = useState(thread.is_resolved);
  const [hunkExpanded, setHunkExpanded] = useState(!thread.is_resolved);
  const [commentsExpanded, setCommentsExpanded] = useState(thread.comments.length <= 2);
  const [replyOpen, setReplyOpen] = useState(false);
  const [replyText, setReplyText] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const first = thread.comments[0];
  const lastComment = thread.comments[thread.comments.length - 1];
  const extraCount = thread.comments.length - 1;
  const visibleComments = commentsExpanded ? thread.comments : thread.comments.slice(0, 1);

  function handleSubmitReply() {
    if (!replyText.trim() || submitting) return;
    setSubmitting(true);
    onReply(thread.id, lastComment.id, replyText.trim());
    setReplyText("");
    setReplyOpen(false);
    setSubmitting(false);
  }

  const lineLabel = thread.line
    ? `L${thread.line}`
    : thread.original_line
      ? `L${thread.original_line} (original)`
      : "";

  return (
    <div className={`thread-card ${thread.is_resolved ? "thread-card-resolved" : ""}`}>
      <div className="thread-card-header" onClick={() => setThreadCollapsed((v) => !v)}>
        <span className={`collapse-chevron ${threadCollapsed ? "collapsed" : ""}`}>&#9662;</span>
        {first?.author.avatar_url && (
          <img className="comment-avatar" src={first.author.avatar_url} alt={first.author.login} width={18} height={18} />
        )}
        <span className="thread-card-author">@{first?.author.login}</span>
        <span className="comment-time">{timeAgo(first?.created_at)}</span>
        <span
          className="thread-card-location"
          onClick={(e) => { e.stopPropagation(); onJumpToThread(thread); }}
          title="Jump to this comment in the diff"
        >
          {lineLabel}
          {thread.is_outdated && <span className="thread-outdated-badge">outdated</span>}
        </span>
        {thread.is_resolved && <span className="thread-resolved-badge">Resolved</span>}
      </div>

      {!threadCollapsed && (
        <>
          {thread.diff_hunk && (
            <div className="thread-hunk-preview">
              <button
                className="thread-hunk-toggle"
                onClick={() => setHunkExpanded((v) => !v)}
              >
                <span className={`collapse-chevron ${hunkExpanded ? "" : "collapsed"}`}>&#9662;</span>
                Diff context
              </button>
              {hunkExpanded && (
                <pre className="thread-hunk-code">{thread.diff_hunk}</pre>
              )}
            </div>
          )}

          <div className="thread-comments">
            {visibleComments.map((comment) => (
              <CommentCard key={comment.id} comment={comment} onEdit={onEdit} onToggleReaction={onToggleReaction} lang={lang} />
            ))}
          </div>

          {!commentsExpanded && extraCount > 1 && (
            <button className="thread-comments-more" onClick={() => setCommentsExpanded(true)}>
              {extraCount} more comment{extraCount === 1 ? "" : "s"}
            </button>
          )}

          <div className="thread-actions">
            {replyOpen ? (
              <div className="thread-reply-form">
                <textarea
                  className="reply-textarea"
                  value={replyText}
                  onChange={(e) => setReplyText(e.target.value)}
                  placeholder="Write a reply..."
                  rows={3}
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                      handleSubmitReply();
                    }
                  }}
                />
                <div className="reply-actions">
                  <button
                    className="reply-cancel"
                    onClick={() => {
                      setReplyOpen(false);
                      setReplyText("");
                    }}
                  >
                    Cancel
                  </button>
                  <button
                    className="reply-submit"
                    disabled={!replyText.trim() || submitting}
                    onClick={handleSubmitReply}
                  >
                    Reply
                  </button>
                </div>
              </div>
            ) : (
              <button className="reply-open-button" onClick={() => setReplyOpen(true)}>
                Reply...
              </button>
            )}
            <button
              className={`resolve-button ${thread.is_resolved ? "resolve-button-resolved" : ""}`}
              onClick={() => onToggleResolved(thread.id, !thread.is_resolved)}
            >
              {thread.is_resolved ? "Unresolve" : "Resolve"}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

/** Always-available compose box for a new PR-level conversation comment.
 * Remounted (keyed on the draft) whenever a new chat draft arrives, so the
 * prefill is mount-only — same pattern as DiffViewer's InlineCommentForm. */
function ConversationComposeForm({
  initialBody,
  onPost,
  onClearDraft,
}: {
  initialBody: string | null;
  onPost: (body: string) => Promise<boolean>;
  onClearDraft: () => void;
}) {
  const [text, setText] = useState(initialBody ?? "");
  const [submitting, setSubmitting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Pre-filled drafts start with the caret at the end so the reviewer can
  // edit immediately; an empty composer is unaffected. Mount-only by design
  // (see the key on this component) — mirrors InlineCommentForm.
  useEffect(() => {
    const ta = textareaRef.current;
    if (ta && ta.value) ta.setSelectionRange(ta.value.length, ta.value.length);
  }, []);

  async function handleSubmit() {
    if (!text.trim() || submitting) return;
    setSubmitting(true);
    const ok = await onPost(text.trim());
    setSubmitting(false);
    if (ok) {
      // Parent also clears the draft on success; this covers the un-prefilled
      // case and resets the box either way.
      setText("");
      if (initialBody !== null) onClearDraft();
    }
    // On failure the text stays — the user shouldn't lose their comment.
  }

  return (
    <div className="thread-reply-form">
      <textarea
        ref={textareaRef}
        className="reply-textarea"
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="Comment on this PR…"
        rows={3}
        autoFocus={initialBody !== null}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) handleSubmit();
        }}
      />
      <div className="reply-actions">
        {initialBody !== null && (
          <button
            className="reply-cancel"
            onClick={() => {
              setText("");
              onClearDraft();
            }}
          >
            Cancel
          </button>
        )}
        <button className="reply-submit" disabled={!text.trim() || submitting} onClick={handleSubmit}>
          Post
        </button>
      </div>
    </div>
  );
}

/** Sort key: unresolved threads first, then by line number. */
function threadSortKey(t: ReviewThread): number {
  return (t.is_resolved ? 1_000_000 : 0) + (t.line ?? t.original_line ?? 0);
}

export function CommentsPanel({
  commentThreads,
  conversation,
  composeInitialBody,
  onPostPrComment,
  onClearComposeDraft,
  onRetry,
  onReply,
  onToggleResolved,
  onEditComment,
  onToggleReaction,
  onClose,
  onOpenFile,
  onJumpToThread,
}: CommentsPanelProps) {
  // null until the first load resolves, so the default (Unresolved, unless
  // there are none) is applied exactly once — after that, the user's choice sticks.
  const [filter, setFilter] = useState<CommentsFilter | null>(null);

  const allThreads = commentThreads.status === "loaded" ? commentThreads.threads : [];
  const unresolvedCount = useMemo(() => allThreads.filter((t) => !t.is_resolved).length, [allThreads]);
  const totalCount = allThreads.length;

  useEffect(() => {
    if (filter !== null) return;
    if (commentThreads.status !== "loaded") return;
    setFilter(unresolvedCount > 0 ? "unresolved" : "all");
  }, [commentThreads.status, unresolvedCount, filter]);

  const effectiveFilter = filter ?? "unresolved";

  const groups = useMemo(() => {
    const filtered = allThreads.filter((t) => effectiveFilter === "all" || !t.is_resolved);
    const byPath = new Map<string, ReviewThread[]>();
    for (const t of filtered) {
      const arr = byPath.get(t.path) ?? [];
      arr.push(t);
      byPath.set(t.path, arr);
    }
    return [...byPath.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([path, threads]) => ({
        path,
        threads: threads.slice().sort((a, b) => threadSortKey(a) - threadSortKey(b)),
      }));
  }, [allThreads, effectiveFilter]);

  return (
    <div className="comments-panel">
      <div className="comments-panel-header">
        <span className="comments-panel-title">Comments</span>
        <button className="chat-icon-btn" onClick={onClose} title="Close comments panel">
          ✕
        </button>
      </div>

      <div className="comments-panel-meta">
        <span className="comments-panel-count">
          {unresolvedCount} unresolved · {totalCount} total
        </span>
        <div className="comments-panel-filter">
          <button
            className={effectiveFilter === "unresolved" ? "active" : ""}
            onClick={() => setFilter("unresolved")}
          >
            Unresolved
          </button>
          <button
            className={effectiveFilter === "all" ? "active" : ""}
            onClick={() => setFilter("all")}
          >
            All
          </button>
        </div>
      </div>

      <div className="comments-panel-body">
        <div className="comments-panel-conversation">
          <div className="comments-panel-section-header">PR conversation</div>
          {conversation !== null && conversation.length === 0 && (
            <div className="comments-panel-empty">No conversation comments yet</div>
          )}
          {conversation?.map((c) => (
            <div key={c.id} className="thread-comment">
              <div className="comment-header">
                <span className="comment-author">@{c.author}</span>
                <span className="comment-time">{timeAgo(c.created_at)}</span>
              </div>
              <CommentBodyRendered body={c.body} />
            </div>
          ))}
          {/* Keyed so a new chat draft remounts the form and takes over the box. */}
          <ConversationComposeForm
            key={composeInitialBody ?? "blank"}
            initialBody={composeInitialBody}
            onPost={onPostPrComment}
            onClearDraft={onClearComposeDraft}
          />
        </div>

        {commentThreads.status === "error" ? (
          <div className="comments-panel-error" role="alert">
            {commentThreads.message}
            <button className="comments-panel-retry" onClick={onRetry}>Retry</button>
          </div>
        ) : commentThreads.status === "loading" || commentThreads.status === "idle" ? (
          <div className="comments-panel-empty">Loading comments…</div>
        ) : totalCount === 0 ? (
          <div className="comments-panel-empty">No review threads on this PR</div>
        ) : groups.length === 0 ? (
          <div className="comments-panel-empty">No unresolved threads — nice work</div>
        ) : (
          groups.map(({ path, threads }) => (
            <div key={path} className="comments-panel-file-group">
              <button
                className="comments-panel-file-header"
                onClick={() => onOpenFile(path)}
                title={path}
              >
                {getFileName(path)}
              </button>
              {threads.map((thread) => (
                <ThreadCard
                  key={thread.id}
                  thread={thread}
                  onReply={onReply}
                  onToggleResolved={onToggleResolved}
                  onEdit={onEditComment}
                  onToggleReaction={onToggleReaction}
                  onJumpToThread={onJumpToThread}
                  lang={detectLanguage(path)}
                />
              ))}
            </div>
          ))
        )}
      </div>
    </div>
  );
}

import { useState, useMemo } from "react";
import type { ReviewThread, ReviewComment } from "../types";
import { timeAgo } from "../utils";
import { CommentBodyRendered, ReactionBar } from "./DiffViewer";

interface CommentsViewerProps {
  threads: ReviewThread[];
  selectedFile: string | null;
  onReply: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved: (threadId: string, resolve: boolean) => void;
  onEditComment?: (commentId: string, body: string) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  lang?: string;
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
  lang,
}: {
  thread: ReviewThread;
  onReply: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved: (threadId: string, resolve: boolean) => void;
  onEdit?: (commentId: string, body: string) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  lang?: string;
}) {
  const [hunkExpanded, setHunkExpanded] = useState(!thread.is_resolved);
  const [replyOpen, setReplyOpen] = useState(false);
  const [replyText, setReplyText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [threadCollapsed, setThreadCollapsed] = useState(thread.is_resolved);

  const lastComment = thread.comments[thread.comments.length - 1];

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
        <span className="thread-card-location">
          {lineLabel}
          {thread.is_outdated && <span className="thread-outdated-badge">outdated</span>}
        </span>
        <span className="thread-card-comment-count">
          {thread.comments.length} comment{thread.comments.length !== 1 ? "s" : ""}
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
            {thread.comments.map((comment) => (
              <CommentCard key={comment.id} comment={comment} onEdit={onEdit} onToggleReaction={onToggleReaction} lang={lang} />
            ))}
          </div>

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

export function CommentsViewer({
  threads,
  selectedFile,
  onReply,
  onToggleResolved,
  onEditComment,
  onToggleReaction,
  lang,
}: CommentsViewerProps) {
  if (!selectedFile) {
    return (
      <div className="comments-viewer-empty">
        Select a file from the Comments sidebar to view review threads
      </div>
    );
  }

  const { fileThreads, unresolvedThreads, resolvedThreads } = useMemo(() => {
    const fileThreads = threads.filter((t) => t.path === selectedFile);
    return {
      fileThreads,
      unresolvedThreads: fileThreads.filter((t) => !t.is_resolved),
      resolvedThreads: fileThreads.filter((t) => t.is_resolved),
    };
  }, [threads, selectedFile]);

  if (fileThreads.length === 0) {
    return (
      <div className="comments-viewer-empty">
        No review threads for this file
      </div>
    );
  }

  return (
    <div className="comments-viewer">
      <div className="comments-viewer-header">
        <span className="comments-viewer-path">{selectedFile}</span>
        <span className="comments-viewer-summary">
          {unresolvedThreads.length} unresolved
          {resolvedThreads.length > 0 && `, ${resolvedThreads.length} resolved`}
        </span>
      </div>
      <div className="comments-viewer-threads">
        {unresolvedThreads.map((thread) => (
          <ThreadCard
            key={thread.id}
            thread={thread}
            onReply={onReply}
            onToggleResolved={onToggleResolved}
            onEdit={onEditComment}
            onToggleReaction={onToggleReaction}
            lang={lang}
          />
        ))}
        {resolvedThreads.map((thread) => (
          <ThreadCard
            key={thread.id}
            thread={thread}
            onReply={onReply}
            onToggleResolved={onToggleResolved}
            onEdit={onEditComment}
            onToggleReaction={onToggleReaction}
            lang={lang}
          />
        ))}
      </div>
    </div>
  );
}

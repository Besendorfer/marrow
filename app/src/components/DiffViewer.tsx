import { Fragment, forwardRef, memo, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import hljs from "highlight.js";
import "highlight.js/styles/github-dark.css";
import type { FileDiff, DiffViewMode, Highlight, ReactionGroup, ReviewThread, ReviewComment, SearchMatch } from "../types";
import { timeAgo, highlightKey } from "../utils";

const extToLang: Record<string, string> = {
  ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript",
  mjs: "javascript", cjs: "javascript", py: "python", rb: "ruby",
  go: "go", rs: "rust", java: "java", kt: "kotlin", swift: "swift",
  c: "c", h: "c", cpp: "cpp", hpp: "cpp", cs: "csharp",
  css: "css", scss: "scss", less: "less", html: "xml", htm: "xml",
  xml: "xml", svg: "xml", vue: "xml", json: "json", yaml: "yaml",
  yml: "yaml", toml: "ini", md: "markdown", sql: "sql", sh: "bash",
  bash: "bash", zsh: "bash", dockerfile: "dockerfile",
  tf: "hcl", hcl: "hcl", graphql: "graphql", gql: "graphql",
};

export function detectLanguage(filePath: string): string | undefined {
  const name = filePath.split("/").pop()?.toLowerCase() ?? "";
  if (name === "dockerfile") return "dockerfile";
  const ext = name.split(".").pop() ?? "";
  return extToLang[ext];
}

function highlightLines(content: string, lang: string | undefined): string[] {
  if (!content) return [];
  let html: string;
  if (lang) {
    try {
      html = hljs.highlight(content, { language: lang }).value;
    } catch {
      html = escapeHtml(content);
    }
  } else {
    html = escapeHtml(content);
  }
  return splitHtmlByLines(html);
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function splitHtmlByLines(html: string): string[] {
  // highlight.js uses \n for line breaks inside its HTML output.
  // We need to split while keeping open <span> tags balanced across lines.
  const lines: string[] = [];
  let current = "";
  const openSpans: string[] = []; // stack of full <span ...> tags

  let i = 0;
  while (i < html.length) {
    if (html[i] === "\n") {
      lines.push(current + openSpans.map(() => "</span>").join(""));
      current = openSpans.join("");
      i++;
    } else if (html[i] === "<") {
      const closeEnd = html.indexOf(">", i);
      if (closeEnd === -1) {
        current += html[i];
        i++;
        continue;
      }
      const tag = html.slice(i, closeEnd + 1);
      if (tag.startsWith("</span>")) {
        openSpans.pop();
        current += tag;
        i = closeEnd + 1;
      } else if (tag.startsWith("<span")) {
        openSpans.push(tag);
        current += tag;
        i = closeEnd + 1;
      } else {
        current += tag;
        i = closeEnd + 1;
      }
    } else {
      current += html[i];
      i++;
    }
  }
  lines.push(current + openSpans.map(() => "</span>").join(""));
  return lines;
}

interface CommentingOn {
  startLine: number;
  endLine: number;
  side: "LEFT" | "RIGHT";
}

interface DiffViewerProps {
  file: FileDiff;
  viewMode: DiffViewMode;
  showHunkSignificance: boolean;
  showAiNotes: boolean;
  /** Keys (highlightKey) of AI highlights dismissed for this PR — hidden from the diff. */
  dismissedHighlights?: Set<string>;
  onToggleHighlightDismissed?: (key: string) => void;
  onCreateComment?: (path: string, endLine: number, side: "LEFT" | "RIGHT", body: string, startLine?: number, startSide?: "LEFT" | "RIGHT") => Promise<void>;
  onEditComment?: (commentId: string, body: string) => void;
  onReply?: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved?: (threadId: string, resolve: boolean) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  reviewThreads?: ReviewThread[];
  searchMatches?: SearchMatch[];
  currentSearchMatch?: SearchMatch | null;
  searchQuery?: string;
}

/** Imperative API for keyboard-driven navigation, folding, and the line cursor. */
export interface DiffViewerHandle {
  nextHunk: () => void;
  prevHunk: () => void;
  /** Toggle fold-all: collapse every hunk, or expand all if any are collapsed. */
  foldAll: () => void;
  // ── Tier 3: line cursor ──────────────────────────────────────────────────
  /** Move the line cursor by `delta` rows (initializing it at the viewport if unset). */
  cursorMove: (delta: number) => void;
  /** Move the line cursor to the first/last navigable line. */
  cursorEdge: (edge: "top" | "bottom") => void;
  /** Move the cursor ~frac of a viewport in `dir` (half page = 0.5, full = 0.9). */
  cursorPage: (dir: 1 | -1, frac: number) => void;
  /** Move the cursor to the next/previous AI finding. */
  nextFinding: () => void;
  prevFinding: () => void;
  /** Fold/unfold the hunk containing the cursor (or the viewport top if no cursor). */
  foldAtCursor: () => void;
  /** Open the comment composer on the cursor line (or the v-selection range). */
  commentAtCursor: () => void;
  /** Toggle the selection anchor at the cursor (extend with cursorMove, then comment). */
  toggleAnchor: () => void;
  /** Reply to the review thread on the cursor line, if any. */
  replyAtCursor: () => void;
  /** Resolve/reopen the review thread on the cursor line, if any. */
  resolveAtCursor: () => void;
}

/** Small keyboard-driven reply box, shown when `r` is pressed on a thread line. */
function KeyboardReplyOverlay({ onSubmit, onClose }: { onSubmit: (body: string) => void; onClose: () => void }) {
  const [text, setText] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { ref.current?.focus(); }, []);
  return (
    <div className="kbd-overlay-backdrop" onClick={onClose}>
      <div className="kbd-overlay" onClick={(e) => e.stopPropagation()}>
        <div className="kbd-overlay-title">Reply to thread</div>
        <textarea
          ref={ref}
          className="kbd-overlay-input"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") { e.preventDefault(); onClose(); }
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && text.trim()) { e.preventDefault(); onSubmit(text.trim()); }
          }}
          placeholder="Write a reply…  (⌘/Ctrl+Enter to send, Esc to cancel)"
        />
      </div>
    </div>
  );
}

export const CommentBodyRendered = memo(function CommentBodyRendered({ body, lang }: { body: string; lang?: string }) {
  // Split body into text and suggestion blocks
  const parts = body.split(/(```suggestion\n[\s\S]*?\n```)/g);

  if (parts.length === 1) {
    // No suggestion blocks — render as plain pre-wrapped text
    return <div className="comment-body">{body}</div>;
  }

  return (
    <div className="comment-body">
      {parts.map((part, i) => {
        const match = part.match(/^```suggestion\n([\s\S]*?)\n```$/);
        if (match) {
          const suggestionCode = match[1];
          const html = highlightCode(suggestionCode, lang);
          return (
            <div key={i} className="comment-suggestion-block">
              <div className="suggestion-preview-header">
                <span className="suggestion-preview-label">Suggestion</span>
              </div>
              <pre className="inline-comment-code suggestion-code-add" dangerouslySetInnerHTML={{ __html: html }} />
            </div>
          );
        }
        if (!part.trim()) return null;
        return <span key={i}>{part}</span>;
      })}
    </div>
  );
});

function highlightCode(code: string, lang: string | undefined): string {
  if (!code) return "";
  if (lang) {
    try { return hljs.highlight(code, { language: lang }).value; } catch { /* fall through */ }
  }
  return escapeHtml(code);
}

function InlineCommentForm({
  onSubmit,
  onCancel,
  colSpan,
  codeSnippet,
  lineRange,
  lang,
}: {
  onSubmit: (body: string) => void;
  onCancel: () => void;
  colSpan: number;
  codeSnippet?: string;
  lineRange?: string;
  lang?: string;
}) {
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  function handleSubmit() {
    if (!text.trim() || submitting) return;
    setSubmitting(true);
    onSubmit(text.trim());
  }

  function handleSuggest() {
    if (!codeSnippet) return;
    const suggestion = "```suggestion\n" + codeSnippet + "\n```\n";
    setText(suggestion);
    requestAnimationFrame(() => {
      const ta = textareaRef.current;
      if (ta) {
        const cursorPos = "```suggestion\n".length;
        ta.focus();
        ta.setSelectionRange(cursorPos, cursorPos + codeSnippet.length);
      }
    });
  }

  // Extract suggestion body from text for live preview
  const suggestionMatch = text.match(/```suggestion\n([\s\S]*?)\n```/);
  const suggestionBody = suggestionMatch?.[1];

  const snippetHtml = codeSnippet ? highlightCode(codeSnippet, lang) : undefined;
  const suggestionHtml = suggestionBody != null ? highlightCode(suggestionBody, lang) : undefined;

  return (
    <tr className="inline-comment-row">
      <td colSpan={colSpan}>
        <div className="inline-comment-form">
          {snippetHtml && !suggestionBody && (
            <div className="inline-comment-snippet">
              {lineRange && <span className="inline-comment-line-range">{lineRange}</span>}
              <pre className="inline-comment-code" dangerouslySetInnerHTML={{ __html: snippetHtml }} />
            </div>
          )}
          {suggestionHtml != null && (
            <div className="inline-comment-suggestion-preview">
              <div className="suggestion-preview-header">
                <span className="suggestion-preview-label">Suggestion</span>
                {lineRange && <span className="inline-comment-line-range">{lineRange}</span>}
              </div>
              {snippetHtml && (
                <div className="suggestion-preview-original">
                  <pre className="inline-comment-code suggestion-code-remove" dangerouslySetInnerHTML={{ __html: snippetHtml }} />
                </div>
              )}
              <div className="suggestion-preview-replacement">
                <pre className="inline-comment-code suggestion-code-add" dangerouslySetInnerHTML={{ __html: suggestionHtml }} />
              </div>
            </div>
          )}
          <textarea
            ref={textareaRef}
            className="reply-textarea"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Add a review comment..."
            rows={5}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                handleSubmit();
              }
              if (e.key === "Escape") {
                onCancel();
              }
            }}
          />
          <div className="reply-actions">
            {codeSnippet && (
              <button className="suggest-button" onClick={handleSuggest} title="Insert a suggestion block with the selected code">
                Suggest
              </button>
            )}
            <button className="reply-cancel" onClick={onCancel}>Cancel</button>
            <button
              className="reply-submit"
              disabled={!text.trim() || submitting}
              onClick={handleSubmit}
            >
              Comment
            </button>
          </div>
        </div>
      </td>
    </tr>
  );
}

function EditableCommentBody({
  comment,
  onEdit,
  lang,
}: {
  comment: ReviewComment;
  onEdit?: (commentId: string, body: string) => void;
  lang?: string;
}) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(comment.body);

  function handleSave() {
    if (!text.trim() || !onEdit) return;
    onEdit(comment.id, text.trim());
    setEditing(false);
  }

  if (editing) {
    return (
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
    );
  }

  return (
    <div className="comment-body-wrapper">
      <CommentBodyRendered body={comment.body} lang={lang} />
      {onEdit && (
        <button className="comment-edit-button" onClick={() => setEditing(true)}>Edit</button>
      )}
    </div>
  );
}

const REACTION_EMOJI: Record<string, string> = {
  THUMBS_UP: "\uD83D\uDC4D",
  THUMBS_DOWN: "\uD83D\uDC4E",
  LAUGH: "\uD83D\uDE04",
  HOORAY: "\uD83C\uDF89",
  CONFUSED: "\uD83D\uDE15",
  HEART: "\u2764\uFE0F",
  ROCKET: "\uD83D\uDE80",
  EYES: "\uD83D\uDC40",
};

const REACTION_CONTENTS = Object.keys(REACTION_EMOJI);

export function ReactionBar({
  reactions,
  commentId,
  onToggleReaction,
}: {
  reactions: ReactionGroup[];
  commentId: string;
  onToggleReaction: (commentId: string, content: string) => void;
}) {
  const [pickerOpen, setPickerOpen] = useState(false);
  const pickerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!pickerOpen) return;
    function handleClickOutside(e: MouseEvent) {
      if (pickerRef.current && !pickerRef.current.contains(e.target as Node)) {
        setPickerOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [pickerOpen]);

  return (
    <div className="reaction-bar">
      {reactions.map((r) => (
        <button
          key={r.content}
          className={`reaction-pill ${r.viewer_has_reacted ? "reaction-pill-active" : ""}`}
          onClick={() => onToggleReaction(commentId, r.content)}
          title={r.content.toLowerCase().replace(/_/g, " ")}
        >
          {REACTION_EMOJI[r.content] ?? r.content} {r.total_count}
        </button>
      ))}
      <div className="reaction-picker-wrapper" ref={pickerRef}>
        <button
          className="reaction-add-button"
          onClick={() => setPickerOpen((v) => !v)}
          title="Add reaction"
        >
          +
        </button>
        {pickerOpen && (
          <div className="reaction-picker">
            {REACTION_CONTENTS.map((content) => (
              <button
                key={content}
                className="reaction-picker-item"
                onClick={() => {
                  onToggleReaction(commentId, content);
                  setPickerOpen(false);
                }}
                title={content.toLowerCase().replace(/_/g, " ")}
              >
                {REACTION_EMOJI[content]}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function InlineThreadMarker({
  thread,
  colSpan,
  onEdit,
  onReply,
  onToggleResolved,
  onToggleReaction,
  lang,
}: {
  thread: ReviewThread;
  colSpan: number;
  onEdit?: (commentId: string, body: string) => void;
  onReply?: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved?: (threadId: string, resolve: boolean) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  lang?: string;
}) {
  const [collapsed, setCollapsed] = useState(thread.is_resolved);
  const [replyOpen, setReplyOpen] = useState(false);
  const [replyText, setReplyText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const isSingle = thread.comments.length === 1;
  const first = thread.comments[0];
  const lastComment = thread.comments[thread.comments.length - 1];

  function handleSubmitReply() {
    if (!replyText.trim() || submitting || !onReply) return;
    setSubmitting(true);
    onReply(thread.id, lastComment.id, replyText.trim());
    setReplyText("");
    setReplyOpen(false);
    setSubmitting(false);
  }

  const showActions = onReply || onToggleResolved;

  return (
    <tr className={`inline-thread-row ${thread.is_resolved ? "inline-thread-resolved" : ""}`}>
      <td colSpan={colSpan}>
        <div className="inline-thread">
          {isSingle ? (
            // Single comment: compact layout, no redundant header+body
            <div className="inline-thread-single">
              <div className="comment-header">
                {first?.author.avatar_url && (
                  <img className="comment-avatar" src={first.author.avatar_url} alt={first.author.login} width={18} height={18} />
                )}
                <span className="comment-author">@{first?.author.login}</span>
                <span className="comment-time">{timeAgo(first?.created_at)}</span>
                {thread.is_resolved && <span className="inline-thread-resolved-badge">Resolved</span>}
              </div>
              <EditableCommentBody comment={first} onEdit={onEdit} lang={lang} />
              {onToggleReaction && <ReactionBar reactions={first?.reactions ?? []} commentId={first?.id} onToggleReaction={onToggleReaction} />}
            </div>
          ) : (
            // Multi-comment: collapsible header + comment list
            <>
              <div className="inline-thread-header" onClick={() => setCollapsed((v) => !v)}>
                <span className={`collapse-chevron ${collapsed ? "collapsed" : ""}`}>&#9662;</span>
                {first?.author.avatar_url && (
                  <img className="comment-avatar" src={first.author.avatar_url} alt={first.author.login} width={16} height={16} />
                )}
                <span className="inline-thread-author">@{first?.author.login}</span>
                <span className="inline-thread-preview">
                  {collapsed ? first?.body.slice(0, 120) : ""}
                </span>
                <span className="inline-thread-meta">
                  <span className="inline-thread-reply-count">
                    {thread.comments.length} comments
                  </span>
                  {thread.is_resolved && <span className="inline-thread-resolved-badge">Resolved</span>}
                  <span className="comment-time">{timeAgo(first?.created_at)}</span>
                </span>
              </div>
              {!collapsed && (
                <div className="inline-thread-comments">
                  {thread.comments.map((comment) => (
                    <div key={comment.id} className="inline-thread-comment">
                      <div className="comment-header">
                        {comment.author.avatar_url && (
                          <img className="comment-avatar" src={comment.author.avatar_url} alt={comment.author.login} width={18} height={18} />
                        )}
                        <span className="comment-author">@{comment.author.login}</span>
                        <span className="comment-time">{timeAgo(comment.created_at)}</span>
                      </div>
                      <EditableCommentBody comment={comment} onEdit={onEdit} lang={lang} />
                      {onToggleReaction && <ReactionBar reactions={comment.reactions ?? []} commentId={comment.id} onToggleReaction={onToggleReaction} />}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
          {showActions && (isSingle || !collapsed) && (
            <div className="thread-actions">
              {onReply && (
                replyOpen ? (
                  <div className="thread-reply-form" style={{ flex: 1 }}>
                    <textarea
                      className="reply-textarea"
                      value={replyText}
                      onChange={(e) => setReplyText(e.target.value)}
                      placeholder="Write a reply..."
                      rows={3}
                      autoFocus
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) handleSubmitReply();
                      }}
                    />
                    <div className="reply-actions">
                      <button className="reply-cancel" onClick={() => { setReplyOpen(false); setReplyText(""); }}>Cancel</button>
                      <button className="reply-submit" disabled={!replyText.trim() || submitting} onClick={handleSubmitReply}>Reply</button>
                    </div>
                  </div>
                ) : (
                  <button className="reply-open-button" onClick={() => setReplyOpen(true)}>Reply...</button>
                )
              )}
              {onToggleResolved && (
                <button
                  className={`resolve-button ${thread.is_resolved ? "resolve-button-resolved" : ""}`}
                  onClick={() => onToggleResolved(thread.id, !thread.is_resolved)}
                >
                  {thread.is_resolved ? "Unresolve" : "Resolve"}
                </button>
              )}
            </div>
          )}
        </div>
      </td>
    </tr>
  );
}

interface DiffLine {
  type: "context" | "add" | "remove" | "header";
  content: string;
  html: string;
  oldLineNum: number | null;
  newLineNum: number | null;
}

function parseDiffLines(
  unifiedDiff: string,
  lang: string | undefined,
  headContent?: string,
  baseContent?: string,
): DiffLine[] {
  const rawLines = unifiedDiff.split("\n");

  // First pass: extract old-side and new-side source lines for highlighting
  const oldSrc: string[] = [];
  const newSrc: string[] = [];
  const entries: { type: DiffLine["type"]; content: string; oldIdx: number | null; newIdx: number | null; oldLineNum: number | null; newLineNum: number | null }[] = [];
  let oldLine = 0;
  let newLine = 0;

  for (const line of rawLines) {
    if (line.startsWith("@@")) {
      const match = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (match) {
        oldLine = parseInt(match[1], 10);
        newLine = parseInt(match[2], 10);
      }
      entries.push({ type: "header", content: line, oldIdx: null, newIdx: null, oldLineNum: null, newLineNum: null });
    } else if (line.startsWith("+")) {
      const content = line.slice(1);
      entries.push({ type: "add", content, oldIdx: null, newIdx: newSrc.length, oldLineNum: null, newLineNum: newLine });
      newSrc.push(content);
      newLine++;
    } else if (line.startsWith("-")) {
      const content = line.slice(1);
      entries.push({ type: "remove", content, oldIdx: oldSrc.length, newIdx: null, oldLineNum: oldLine, newLineNum: null });
      oldSrc.push(content);
      oldLine++;
    } else if (line.startsWith("\\")) {
      // skip
    } else {
      const content = line.startsWith(" ") ? line.slice(1) : line;
      entries.push({ type: "context", content, oldIdx: oldSrc.length, newIdx: newSrc.length, oldLineNum: oldLine, newLineNum: newLine });
      oldSrc.push(content);
      newSrc.push(content);
      oldLine++;
      newLine++;
    }
  }

  // Highlight from the COMPLETE file contents when available, indexing each diff
  // line by its real line number. The diff reconstruction (oldSrc/newSrc) stitches
  // non-contiguous hunks together — that's syntactically broken code, and on a
  // large/scattered diff it breaks highlight.js's stateful lexer so most tokens
  // render uncolored. The full files are valid contiguous code and highlight
  // correctly. Fall back to the stitched reconstruction for a side whose full
  // content isn't available (e.g. a renamed file's missing base), preserving the
  // previous behavior there.
  const newFull = headContent ? highlightLines(headContent, lang) : null;
  const oldFull = baseContent ? highlightLines(baseContent, lang) : null;
  const newStitched = newFull ? null : highlightLines(newSrc.join("\n"), lang);
  const oldStitched = oldFull ? null : highlightLines(oldSrc.join("\n"), lang);

  const newHtmlOf = (e: (typeof entries)[number]): string | undefined =>
    newFull
      ? (e.newLineNum != null ? newFull[e.newLineNum - 1] : undefined)
      : (e.newIdx != null ? newStitched![e.newIdx] : undefined);
  const oldHtmlOf = (e: (typeof entries)[number]): string | undefined =>
    oldFull
      ? (e.oldLineNum != null ? oldFull[e.oldLineNum - 1] : undefined)
      : (e.oldIdx != null ? oldStitched![e.oldIdx] : undefined);

  // Map highlighted HTML back to diff lines (add + context use the new side).
  return entries.map((e) => {
    const html =
      e.type === "header"
        ? escapeHtml(e.content)
        : (e.type === "remove" ? oldHtmlOf(e) : newHtmlOf(e)) ?? escapeHtml(e.content);
    return { type: e.type, content: e.content, html, oldLineNum: e.oldLineNum, newLineNum: e.newLineNum };
  });
}

interface SplitLine {
  left: DiffLine | null;
  right: DiffLine | null;
}

function buildSplitLines(diffLines: DiffLine[]): SplitLine[] {
  const result: SplitLine[] = [];
  let i = 0;

  while (i < diffLines.length) {
    const line = diffLines[i];

    if (line.type === "header") {
      result.push({ left: line, right: line });
      i++;
      continue;
    }

    if (line.type === "context") {
      result.push({ left: line, right: line });
      i++;
      continue;
    }

    // Collect consecutive removes and adds for pairing
    if (line.type === "remove") {
      const removes: DiffLine[] = [];
      while (i < diffLines.length && diffLines[i].type === "remove") {
        removes.push(diffLines[i]);
        i++;
      }
      const adds: DiffLine[] = [];
      while (i < diffLines.length && diffLines[i].type === "add") {
        adds.push(diffLines[i]);
        i++;
      }

      const maxLen = Math.max(removes.length, adds.length);
      for (let j = 0; j < maxLen; j++) {
        result.push({
          left: j < removes.length ? removes[j] : null,
          right: j < adds.length ? adds[j] : null,
        });
      }
      continue;
    }

    if (line.type === "add") {
      result.push({ left: null, right: line });
      i++;
      continue;
    }

    i++;
  }

  return result;
}

function buildFullFileLines(content: string, type: "add" | "remove", lang: string | undefined): DiffLine[] {
  if (!content) return [];
  const lines = content.split("\n");
  const htmlLines = highlightLines(content, lang);
  return lines.map((line, idx) => ({
    type,
    content: line,
    html: htmlLines[idx] ?? escapeHtml(line),
    oldLineNum: type === "remove" ? idx + 1 : null,
    newLineNum: type === "add" ? idx + 1 : null,
  }));
}

// Expand a modified file's diff to the WHOLE file: keep the real additions and
// removals (and their split/unified rendering), and fill in every unchanged
// line from head_content as context — like GitHub's "expand all". Produces no
// `@@` headers, so groupIntoHunks yields a single hunk the normal renderer
// shows without a hunk header.
function buildFullFileDiffLines(
  headContent: string,
  unifiedDiff: string,
  lang: string | undefined
): DiffLine[] {
  if (!headContent) return [];
  const parsed = parseDiffLines(unifiedDiff, lang, headContent);
  const newLines = headContent.split("\n");
  const newHtml = highlightLines(headContent, lang);

  const contextLine = (newLineNum: number, oldLineNum: number): DiffLine => ({
    type: "context",
    content: newLines[newLineNum - 1] ?? "",
    html: newHtml[newLineNum - 1] ?? escapeHtml(newLines[newLineNum - 1] ?? ""),
    oldLineNum,
    newLineNum,
  });

  const out: DiffLine[] = [];
  let nextNew = 1; // next unchanged new-side line to emit
  let nextOld = 1; // its old-side counterpart (they advance together in gaps)

  for (const e of parsed) {
    if (e.type === "header") {
      const m = e.content.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      const newStart = m ? parseInt(m[2], 10) : nextNew;
      while (nextNew < newStart) {
        out.push(contextLine(nextNew, nextOld));
        nextNew++;
        nextOld++;
      }
      continue; // don't emit the @@ header itself
    }
    out.push(e);
    if (e.type === "add") {
      nextNew = (e.newLineNum ?? nextNew) + 1;
    } else if (e.type === "remove") {
      nextOld = (e.oldLineNum ?? nextOld) + 1;
    } else {
      nextNew = (e.newLineNum ?? nextNew) + 1;
      nextOld = (e.oldLineNum ?? nextOld) + 1;
    }
  }

  while (nextNew <= newLines.length) {
    out.push(contextLine(nextNew, nextOld));
    nextNew++;
    nextOld++;
  }
  return out;
}

// Shared empty set for "nothing collapsed" (full-file view never collapses).
const NO_COLLAPSED: Set<number> = new Set();

const LINE_TINT_THRESHOLD = 50;

function getHighlightForLine(
  lineNum: number | null,
  highlights: Highlight[]
): Highlight | undefined {
  if (lineNum === null || highlights.length === 0) return undefined;
  return highlights.find(
    (h) =>
      lineNum >= h.start_line &&
      lineNum <= h.end_line &&
      h.end_line - h.start_line + 1 <= LINE_TINT_THRESHOLD
  );
}

function isHighlightStart(
  lineNum: number | null,
  highlights: Highlight[]
): Highlight | undefined {
  if (lineNum === null || highlights.length === 0) return undefined;
  return highlights.find((h) => lineNum === h.start_line);
}

function formatLineRange(start: number, end: number): string {
  return `L${start}${end !== start ? `\u2013${end}` : ""}`;
}

const severityIcon: Record<string, string> = {
  critical: "!!",
  warning: "!",
  info: "i",
};

function HighlightMarker({ highlight, onPostAsComment }: { highlight: Highlight; onPostAsComment?: (h: Highlight) => void }) {
  return (
    <div className={`highlight-marker highlight-${highlight.severity}`}>
      <span className="highlight-icon">
        {severityIcon[highlight.severity] || "i"}
      </span>
      <span className="highlight-lines">{formatLineRange(highlight.start_line, highlight.end_line)}</span>
      <span className="highlight-comment">{highlight.comment}</span>
      {onPostAsComment && (
        <button
          className="highlight-post-comment"
          onClick={(e) => { e.stopPropagation(); onPostAsComment(highlight); }}
          title="Post this AI note as a review comment"
        >
          Post as comment
        </button>
      )}
    </div>
  );
}

// ── Hunk grouping ────────────────────────────────────────────────────────

interface Hunk {
  index: number;
  headerLine: DiffLine | null;
  lines: DiffLine[];
  significance: string;
  lineCount: number;
}

function groupIntoHunks(
  diffLines: DiffLine[],
  hunkScores: string[]
): Hunk[] {
  const hunks: Hunk[] = [];
  let currentLines: DiffLine[] = [];
  let currentHeader: DiffLine | null = null;
  let hunkIdx = -1;

  for (const line of diffLines) {
    if (line.type === "header") {
      // Save previous hunk
      if (hunkIdx >= 0 || currentLines.length > 0) {
        hunks.push({
          index: Math.max(hunkIdx, 0),
          headerLine: currentHeader,
          lines: currentLines,
          significance: hunkScores[Math.max(hunkIdx, 0)] ?? "medium",
          lineCount: currentLines.length,
        });
      }
      hunkIdx++;
      currentHeader = line;
      currentLines = [];
    } else {
      currentLines.push(line);
    }
  }

  // Save last hunk
  if (currentLines.length > 0 || currentHeader) {
    hunks.push({
      index: Math.max(hunkIdx, 0),
      headerLine: currentHeader,
      lines: currentLines,
      significance: hunkScores[Math.max(hunkIdx, 0)] ?? "medium",
      lineCount: currentLines.length,
    });
  }

  return hunks;
}

function buildThreadsByLine(threads: ReviewThread[] | undefined): Map<number, ReviewThread[]> {
  const map = new Map<number, ReviewThread[]>();
  for (const t of threads ?? []) {
    const line = t.line ?? t.original_line;
    if (line == null) continue;
    const arr = map.get(line);
    if (arr) arr.push(t);
    else map.set(line, [t]);
  }
  return map;
}

// ── Unified hunk rendering ───────────────────────────────────────────────

function UnifiedHunkLines({
  lines,
  highlights,
  commentingOn,
  dragging,
  onLineMouseDown,
  onLineMouseEnter,
  onLineMouseUp,
  onSubmitComment,
  onCancelComment,
  reviewThreads,
  onEditComment,
  onReply,
  onToggleResolved,
  onToggleReaction,
  onPostHighlightAsComment,
  lang,
}: {
  lines: DiffLine[];
  highlights: Highlight[];
  commentingOn?: CommentingOn | null;
  dragging?: { anchorLine: number; side: "LEFT" | "RIGHT"; currentLine: number } | null;
  onLineMouseDown?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseEnter?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseUp?: () => void;
  onSubmitComment?: (body: string) => void;
  onCancelComment?: () => void;
  reviewThreads?: ReviewThread[];
  onEditComment?: (commentId: string, body: string) => void;
  onReply?: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved?: (threadId: string, resolve: boolean) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  onPostHighlightAsComment?: (h: Highlight) => void;
  lang?: string;
}) {
  const threadsByLine = useMemo(() => buildThreadsByLine(reviewThreads), [reviewThreads]);

  return (
    <>
      {lines.map((line, idx) => {
        const lineNum = line.newLineNum ?? line.oldLineNum;
        const hl = getHighlightForLine(lineNum, highlights);
        const hlStart = isHighlightStart(lineNum, highlights);
        const commentLine = line.type === "remove" ? line.oldLineNum : line.newLineNum;
        const commentSide: "LEFT" | "RIGHT" = line.type === "remove" ? "LEFT" : "RIGHT";
        const canComment = onLineMouseDown && line.type !== "header" && commentLine != null;
        const isEndOfSelection =
          commentingOn &&
          commentingOn.endLine === commentLine &&
          commentingOn.side === commentSide;
        const isInSelection =
          (commentingOn &&
            commentingOn.side === commentSide &&
            commentLine != null &&
            commentLine >= commentingOn.startLine &&
            commentLine <= commentingOn.endLine) ||
          (dragging &&
            dragging.side === commentSide &&
            commentLine != null &&
            commentLine >= Math.min(dragging.anchorLine, dragging.currentLine) &&
            commentLine <= Math.max(dragging.anchorLine, dragging.currentLine));
        const lineThreads = commentLine != null ? (threadsByLine.get(commentLine) ?? []) : [];
        // Build code snippet for the comment form at end of selection
        const codeSnippet = isEndOfSelection && commentingOn
          ? lines
              .filter((l) => {
                const ln = l.type === "remove" ? l.oldLineNum : l.newLineNum;
                return ln != null && ln >= commentingOn.startLine && ln <= commentingOn.endLine && l.type !== "header";
              })
              .map((l) => l.content)
              .join("\n")
          : undefined;
        const lineRange = isEndOfSelection && commentingOn && commentingOn.startLine !== commentingOn.endLine
          ? `L${commentingOn.startLine}-L${commentingOn.endLine}`
          : undefined;

        return (
          <Fragment key={idx}>
            {hlStart && (
              <tr className="highlight-row">
                <td colSpan={4}>
                  <HighlightMarker highlight={hlStart} onPostAsComment={onPostHighlightAsComment} />
                </td>
              </tr>
            )}
            <tr
              id={lineNum != null ? `diff-line-${lineNum}` : undefined}
              data-cl={commentLine ?? undefined}
              data-cs={commentLine != null ? commentSide : undefined}
              className={`diff-line diff-line-${line.type}${hl ? ` highlighted highlighted-${hl.severity}` : ""}${canComment ? " commentable-line" : ""}${isInSelection ? " line-selected" : ""}`}
            >
              <td
                className="line-num"
                onMouseDown={canComment && line.type !== "add" ? (e) => { e.preventDefault(); onLineMouseDown(commentLine!, commentSide); } : undefined}
                onMouseEnter={canComment && onLineMouseEnter && line.type !== "add" ? () => onLineMouseEnter(commentLine!, commentSide) : undefined}
                onMouseUp={canComment && onLineMouseUp && line.type !== "add" ? onLineMouseUp : undefined}
              >
                {line.oldLineNum ?? ""}
                {canComment && line.type !== "add" && (
                  <span className="line-comment-button">+</span>
                )}
              </td>
              <td
                className="line-num"
                onMouseDown={canComment && line.type !== "remove" ? (e) => { e.preventDefault(); onLineMouseDown(commentLine!, commentSide); } : undefined}
                onMouseEnter={canComment && onLineMouseEnter && line.type !== "remove" ? () => onLineMouseEnter(commentLine!, commentSide) : undefined}
                onMouseUp={canComment && onLineMouseUp && line.type !== "remove" ? onLineMouseUp : undefined}
              >
                {line.newLineNum ?? ""}
                {canComment && line.type !== "remove" && (
                  <span className="line-comment-button">+</span>
                )}
              </td>
              <td className="line-prefix">
                {line.type === "add" ? "+" : line.type === "remove" ? "-" : line.type === "header" ? "@@" : " "}
              </td>
              <td className="line-content">
                <pre dangerouslySetInnerHTML={{ __html: line.html }} />
              </td>
            </tr>
            {lineThreads.map((thread) => (
              <InlineThreadMarker key={thread.id} thread={thread} colSpan={4} onEdit={onEditComment} onReply={onReply} onToggleResolved={onToggleResolved} onToggleReaction={onToggleReaction} lang={lang} />
            ))}
            {isEndOfSelection && onSubmitComment && onCancelComment && (
              <InlineCommentForm
                onSubmit={onSubmitComment}
                onCancel={onCancelComment}
                colSpan={4}
                codeSnippet={codeSnippet}
                lineRange={lineRange}
                lang={lang}
              />
            )}
          </Fragment>
        );
      })}
    </>
  );
}

function UnifiedView({
  hunks,
  highlights,
  collapsedHunks,
  onToggleHunk,
  showSignificance,
  commentingOn,
  dragging,
  onLineMouseDown,
  onLineMouseEnter,
  onLineMouseUp,
  onSubmitComment,
  onCancelComment,
  reviewThreads,
  onEditComment,
  onReply,
  onToggleResolved,
  onToggleReaction,
  onPostHighlightAsComment,
  lang,
}: {
  hunks: Hunk[];
  highlights: Highlight[];
  collapsedHunks: Set<number>;
  onToggleHunk: (index: number) => void;
  showSignificance: boolean;
  commentingOn?: CommentingOn | null;
  dragging?: { anchorLine: number; side: "LEFT" | "RIGHT"; currentLine: number } | null;
  onLineMouseDown?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseEnter?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseUp?: () => void;
  onSubmitComment?: (body: string) => void;
  onCancelComment?: () => void;
  reviewThreads?: ReviewThread[];
  onEditComment?: (commentId: string, body: string) => void;
  onReply?: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved?: (threadId: string, resolve: boolean) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  onPostHighlightAsComment?: (h: Highlight) => void;
  lang?: string;
}) {
  return (
    <table className="diff-table unified">
      <colgroup>
        <col style={{ width: 50 }} />
        <col style={{ width: 50 }} />
        <col style={{ width: 20 }} />
        <col />
      </colgroup>
      <tbody>
        {hunks.map((hunk) => {
          const isLow = showSignificance && hunk.significance === "low";
          const isHigh = showSignificance && hunk.significance === "high";
          const isCollapsed = collapsedHunks.has(hunk.index);
          const isDimmed = isLow && !isCollapsed;

          return (
            <Fragment key={hunk.index}>
              {showSignificance && hunk.headerLine && (
                <tr
                  id={`hunk-${hunk.index}`}
                  data-cl={isCollapsed ? hunk.index : undefined}
                  data-cs={isCollapsed ? "FOLD" : undefined}
                  className={`diff-line diff-line-header${isHigh ? " hunk-header-high" : ""} hunk-header-clickable`}
                  onClick={() => onToggleHunk(hunk.index)}
                >
                  <td className="line-num"></td>
                  <td className="line-num"></td>
                  <td className="line-prefix">@@</td>
                  <td className="line-content">
                    <pre dangerouslySetInnerHTML={{ __html: escapeHtml(hunk.headerLine.content) }} />
                    {isHigh && <span className="hunk-significance-badge hunk-badge-high">HIGH</span>}
                    {isCollapsed && <span className="hunk-collapsed-indicator">{hunk.lineCount} lines</span>}
                  </td>
                </tr>
              )}
              {isCollapsed ? (
                !hunk.headerLine && (
                  <tr
                    id={`hunk-${hunk.index}`}
                    data-cl={hunk.index}
                    data-cs="FOLD"
                    className="hunk-collapsed"
                    onClick={() => onToggleHunk(hunk.index)}
                  >
                    <td colSpan={4}>
                      <span className="hunk-collapsed-chevron">&#9654;</span>
                      {hunk.lineCount} lines collapsed (click to expand)
                    </td>
                  </tr>
                )
              ) : isDimmed ? (
                <tr className="hunk-low-significance">
                  <td colSpan={4} style={{ padding: 0 }}>
                    <table className="diff-table unified hunk-low-significance-inner">
                      <colgroup>
                        <col style={{ width: 50 }} />
                        <col style={{ width: 50 }} />
                        <col style={{ width: 20 }} />
                        <col />
                      </colgroup>
                      <tbody>
                        <UnifiedHunkLines lines={hunk.lines} highlights={highlights} commentingOn={commentingOn} dragging={dragging} onLineMouseDown={onLineMouseDown} onLineMouseEnter={onLineMouseEnter} onLineMouseUp={onLineMouseUp} onSubmitComment={onSubmitComment} onCancelComment={onCancelComment} reviewThreads={reviewThreads} onEditComment={onEditComment} onReply={onReply} onToggleResolved={onToggleResolved} onToggleReaction={onToggleReaction} onPostHighlightAsComment={onPostHighlightAsComment} lang={lang} />
                      </tbody>
                    </table>
                  </td>
                </tr>
              ) : (
                <UnifiedHunkLines lines={hunk.lines} highlights={highlights} commentingOn={commentingOn} dragging={dragging} onLineMouseDown={onLineMouseDown} onLineMouseEnter={onLineMouseEnter} onLineMouseUp={onLineMouseUp} onSubmitComment={onSubmitComment} onCancelComment={onCancelComment} reviewThreads={reviewThreads} onEditComment={onEditComment} onReply={onReply} onToggleResolved={onToggleResolved} onToggleReaction={onToggleReaction} onPostHighlightAsComment={onPostHighlightAsComment} lang={lang} />
              )}
            </Fragment>
          );
        })}
      </tbody>
    </table>
  );
}

// ── Split hunk rendering ─────────────────────────────────────────────────

function SplitHunkLines({
  splitLines,
  highlights,
  commentingOn,
  dragging,
  onLineMouseDown,
  onLineMouseEnter,
  onLineMouseUp,
  onSubmitComment,
  onCancelComment,
  reviewThreads,
  onEditComment,
  onReply,
  onToggleResolved,
  onToggleReaction,
  onPostHighlightAsComment,
  lang,
}: {
  splitLines: SplitLine[];
  highlights: Highlight[];
  commentingOn?: CommentingOn | null;
  dragging?: { anchorLine: number; side: "LEFT" | "RIGHT"; currentLine: number } | null;
  onLineMouseDown?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseEnter?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseUp?: () => void;
  onSubmitComment?: (body: string) => void;
  onCancelComment?: () => void;
  reviewThreads?: ReviewThread[];
  onEditComment?: (commentId: string, body: string) => void;
  onReply?: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved?: (threadId: string, resolve: boolean) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  onPostHighlightAsComment?: (h: Highlight) => void;
  lang?: string;
}) {
  const threadsByLine = useMemo(() => buildThreadsByLine(reviewThreads), [reviewThreads]);

  return (
    <>
      {splitLines.map((pair, idx) => {
        const rightLineNum = pair.right?.newLineNum ?? pair.right?.oldLineNum;
        const hl = getHighlightForLine(rightLineNum ?? null, highlights);
        const hlStart = isHighlightStart(rightLineNum ?? null, highlights);
        const leftLine = pair.left?.oldLineNum ?? pair.left?.newLineNum ?? null;
        const rightLine = pair.right?.newLineNum ?? pair.right?.oldLineNum ?? null;
        const canCommentLeft = onLineMouseDown && pair.left && pair.left.type !== "header" && leftLine != null;
        const canCommentRight = onLineMouseDown && pair.right && pair.right.type !== "header" && rightLine != null;
        const leftSide: "LEFT" | "RIGHT" = pair.left?.type === "remove" ? "LEFT" : "RIGHT";
        const rightSide: "LEFT" | "RIGHT" = "RIGHT";
        // The row's primary keyboard-cursor target: prefer the new (right) side,
        // falling back to the old (left) side for pure removals.
        const curCl = canCommentRight ? rightLine : (canCommentLeft ? leftLine : null);
        const curCs: "LEFT" | "RIGHT" = canCommentRight ? rightSide : leftSide;
        const isEndLeft =
          commentingOn && commentingOn.endLine === leftLine && commentingOn.side === leftSide;
        const isEndRight =
          commentingOn && commentingOn.endLine === rightLine && commentingOn.side === rightSide;
        const showForm = isEndLeft || isEndRight;
        // Build code snippet for the comment form at end of selection
        const codeSnippet = showForm && commentingOn
          ? splitLines
              .map((p) => {
                const dl = commentingOn.side === "LEFT" ? p.left : p.right;
                if (!dl || dl.type === "header") return null;
                const ln = dl.type === "remove" ? dl.oldLineNum : dl.newLineNum;
                if (ln == null || ln < commentingOn.startLine || ln > commentingOn.endLine) return null;
                return dl.content;
              })
              .filter((c): c is string => c !== null)
              .join("\n")
          : undefined;
        const lineRange = showForm && commentingOn && commentingOn.startLine !== commentingOn.endLine
          ? `L${commentingOn.startLine}-L${commentingOn.endLine}`
          : undefined;

        const isInSelection = (ln: number | null, side: "LEFT" | "RIGHT") => {
          if (ln == null) return false;
          if (commentingOn && commentingOn.side === side && ln >= commentingOn.startLine && ln <= commentingOn.endLine) return true;
          if (dragging && dragging.side === side && ln >= Math.min(dragging.anchorLine, dragging.currentLine) && ln <= Math.max(dragging.anchorLine, dragging.currentLine)) return true;
          return false;
        };

        // Collect threads for both sides, deduplicated
        const leftThreads = leftLine != null ? (threadsByLine.get(leftLine) ?? []) : [];
        const rightThreads = rightLine != null ? (threadsByLine.get(rightLine) ?? []) : [];
        const lineThreads = rightLine === leftLine
          ? leftThreads
          : [...leftThreads, ...rightThreads.filter(t => !leftThreads.some(lt => lt.id === t.id))];

        return (
          <Fragment key={idx}>
            {hlStart && (
              <tr className="highlight-row">
                <td colSpan={4}>
                  <HighlightMarker highlight={hlStart} onPostAsComment={onPostHighlightAsComment} />
                </td>
              </tr>
            )}
            <tr
              id={rightLineNum != null ? `diff-line-${rightLineNum}` : undefined}
              data-cl={curCl ?? undefined}
              data-cs={curCl != null ? curCs : undefined}
              className={`diff-split-row${hl ? ` highlighted highlighted-${hl.severity}` : ""}${(canCommentLeft || canCommentRight) ? " commentable-line" : ""}`}
            >
              {/* Left side (base/old) */}
              <td
                className={`line-num left-num${isInSelection(leftLine, leftSide) ? " line-selected" : ""}`}
                onMouseDown={canCommentLeft ? (e) => { e.preventDefault(); onLineMouseDown(leftLine!, leftSide); } : undefined}
                onMouseEnter={canCommentLeft && onLineMouseEnter ? () => onLineMouseEnter(leftLine!, leftSide) : undefined}
                onMouseUp={canCommentLeft && onLineMouseUp ? onLineMouseUp : undefined}
              >
                {leftLine ?? ""}
                {canCommentLeft && <span className="line-comment-button">+</span>}
              </td>
              <td
                className={`line-content left-content ${pair.left ? `diff-line-${pair.left.type}` : "empty-line"}${isInSelection(leftLine, leftSide) ? " line-selected" : ""}`}
              >
                <pre dangerouslySetInnerHTML={{ __html: pair.left?.html ?? "" }} />
              </td>
              {/* Right side (head/new) */}
              <td
                className={`line-num right-num${isInSelection(rightLine, rightSide) ? " line-selected" : ""}`}
                onMouseDown={canCommentRight ? (e) => { e.preventDefault(); onLineMouseDown(rightLine!, rightSide); } : undefined}
                onMouseEnter={canCommentRight && onLineMouseEnter ? () => onLineMouseEnter(rightLine!, rightSide) : undefined}
                onMouseUp={canCommentRight && onLineMouseUp ? onLineMouseUp : undefined}
              >
                {rightLine ?? ""}
                {canCommentRight && <span className="line-comment-button">+</span>}
              </td>
              <td
                className={`line-content right-content ${pair.right ? `diff-line-${pair.right.type}` : "empty-line"}${isInSelection(rightLine, rightSide) ? " line-selected" : ""}`}
              >
                <pre dangerouslySetInnerHTML={{ __html: pair.right?.html ?? "" }} />
              </td>
            </tr>
            {lineThreads.map((thread) => (
              <InlineThreadMarker key={thread.id} thread={thread} colSpan={4} onEdit={onEditComment} onReply={onReply} onToggleResolved={onToggleResolved} onToggleReaction={onToggleReaction} lang={lang} />
            ))}
            {showForm && onSubmitComment && onCancelComment && (
              <InlineCommentForm
                onSubmit={onSubmitComment}
                onCancel={onCancelComment}
                colSpan={4}
                codeSnippet={codeSnippet}
                lineRange={lineRange}
                lang={lang}
              />
            )}
          </Fragment>
        );
      })}
    </>
  );
}

function SplitView({
  hunks,
  highlights,
  collapsedHunks,
  onToggleHunk,
  showSignificance,
  commentingOn,
  dragging,
  onLineMouseDown,
  onLineMouseEnter,
  onLineMouseUp,
  onSubmitComment,
  onCancelComment,
  reviewThreads,
  onEditComment,
  onReply,
  onToggleResolved,
  onToggleReaction,
  onPostHighlightAsComment,
  lang,
}: {
  hunks: Hunk[];
  highlights: Highlight[];
  collapsedHunks: Set<number>;
  onToggleHunk: (index: number) => void;
  showSignificance: boolean;
  commentingOn?: CommentingOn | null;
  dragging?: { anchorLine: number; side: "LEFT" | "RIGHT"; currentLine: number } | null;
  onLineMouseDown?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseEnter?: (line: number, side: "LEFT" | "RIGHT") => void;
  onLineMouseUp?: () => void;
  onSubmitComment?: (body: string) => void;
  onCancelComment?: () => void;
  reviewThreads?: ReviewThread[];
  onEditComment?: (commentId: string, body: string) => void;
  onReply?: (threadId: string, commentId: string, body: string) => void;
  onToggleResolved?: (threadId: string, resolve: boolean) => void;
  onToggleReaction?: (commentId: string, content: string) => void;
  onPostHighlightAsComment?: (h: Highlight) => void;
  lang?: string;
}) {
  return (
    <table className="diff-table split">
      <colgroup>
        <col style={{ width: 50 }} />
        <col />
        <col style={{ width: 50 }} />
        <col />
      </colgroup>
      <tbody>
        {hunks.map((hunk) => {
          const isLow = showSignificance && hunk.significance === "low";
          const isHigh = showSignificance && hunk.significance === "high";
          const isCollapsed = collapsedHunks.has(hunk.index);
          const isDimmed = isLow && !isCollapsed;
          const splitLines = isCollapsed ? [] : buildSplitLines(hunk.lines);

          return (
            <Fragment key={hunk.index}>
              {showSignificance && hunk.headerLine && (
                <tr
                  id={`hunk-${hunk.index}`}
                  data-cl={isCollapsed ? hunk.index : undefined}
                  data-cs={isCollapsed ? "FOLD" : undefined}
                  className={`diff-split-row diff-line-header${isHigh ? " hunk-header-high" : ""} hunk-header-clickable`}
                  onClick={() => onToggleHunk(hunk.index)}
                >
                  <td className="line-num left-num"></td>
                  <td className="line-content left-content diff-line-header">
                    <pre dangerouslySetInnerHTML={{ __html: escapeHtml(hunk.headerLine.content) }} />
                  </td>
                  <td className="line-num right-num"></td>
                  <td className="line-content right-content diff-line-header">
                    <pre dangerouslySetInnerHTML={{ __html: escapeHtml(hunk.headerLine.content) }} />
                    {isHigh && <span className="hunk-significance-badge hunk-badge-high">HIGH</span>}
                    {isCollapsed && <span className="hunk-collapsed-indicator">{hunk.lineCount} lines</span>}
                  </td>
                </tr>
              )}
              {isCollapsed ? (
                !hunk.headerLine && (
                  <tr
                    id={`hunk-${hunk.index}`}
                    data-cl={hunk.index}
                    data-cs="FOLD"
                    className="hunk-collapsed"
                    onClick={() => onToggleHunk(hunk.index)}
                  >
                    <td colSpan={4}>
                      <span className="hunk-collapsed-chevron">&#9654;</span>
                      {hunk.lineCount} lines collapsed (click to expand)
                    </td>
                  </tr>
                )
              ) : isDimmed ? (
                <tr className="hunk-low-significance">
                  <td colSpan={4} style={{ padding: 0 }}>
                    <table className="diff-table split hunk-low-significance-inner">
                      <colgroup>
                        <col style={{ width: 50 }} />
                        <col />
                        <col style={{ width: 50 }} />
                        <col />
                      </colgroup>
                      <tbody>
                        <SplitHunkLines splitLines={splitLines} highlights={highlights} commentingOn={commentingOn} dragging={dragging} onLineMouseDown={onLineMouseDown} onLineMouseEnter={onLineMouseEnter} onLineMouseUp={onLineMouseUp} onSubmitComment={onSubmitComment} onCancelComment={onCancelComment} reviewThreads={reviewThreads} onEditComment={onEditComment} onReply={onReply} onToggleResolved={onToggleResolved} onToggleReaction={onToggleReaction} onPostHighlightAsComment={onPostHighlightAsComment} lang={lang} />
                      </tbody>
                    </table>
                  </td>
                </tr>
              ) : (
                <SplitHunkLines splitLines={splitLines} highlights={highlights} commentingOn={commentingOn} dragging={dragging} onLineMouseDown={onLineMouseDown} onLineMouseEnter={onLineMouseEnter} onLineMouseUp={onLineMouseUp} onSubmitComment={onSubmitComment} onCancelComment={onCancelComment} reviewThreads={reviewThreads} onEditComment={onEditComment} onReply={onReply} onToggleResolved={onToggleResolved} onToggleReaction={onToggleReaction} onPostHighlightAsComment={onPostHighlightAsComment} lang={lang} />
              )}
            </Fragment>
          );
        })}
      </tbody>
    </table>
  );
}

// ── Main DiffViewer ──────────────────────────────────────────────────────

export const DiffViewer = forwardRef<DiffViewerHandle, DiffViewerProps>(function DiffViewer({ file, viewMode, showHunkSignificance, showAiNotes, dismissedHighlights, onToggleHighlightDismissed, onCreateComment, onEditComment, onReply, onToggleResolved, onToggleReaction, reviewThreads, searchMatches, currentSearchMatch: currentMatchInFile, searchQuery }: DiffViewerProps, ref) {
  const [commentingOn, setCommentingOn] = useState<CommentingOn | null>(null);
  const [dragging, setDragging] = useState<{ anchorLine: number; side: "LEFT" | "RIGHT"; currentLine: number } | null>(null);
  // "View full file" toggle (modified files). Resets per file via the key prop.
  const [fullFile, setFullFile] = useState(false);
  const diffContentRef = useRef<HTMLDivElement>(null);

  function handleLineMouseDown(line: number, side: "LEFT" | "RIGHT") {
    setDragging({ anchorLine: line, side, currentLine: line });
    setCommentingOn(null);
  }

  function handleLineMouseEnter(line: number, side: "LEFT" | "RIGHT") {
    if (!dragging || dragging.side !== side) return;
    setDragging((d) => d ? { ...d, currentLine: line } : null);
  }

  function handleLineMouseUp() {
    if (!dragging) return;
    const startLine = Math.min(dragging.anchorLine, dragging.currentLine);
    const endLine = Math.max(dragging.anchorLine, dragging.currentLine);
    setCommentingOn({ startLine, endLine, side: dragging.side });
    setDragging(null);
  }

  function handleSubmitComment(body: string) {
    if (!commentingOn || !onCreateComment) return;
    const isRange = commentingOn.startLine !== commentingOn.endLine;
    onCreateComment(
      file.path,
      commentingOn.endLine,
      commentingOn.side,
      body,
      isRange ? commentingOn.startLine : undefined,
      isRange ? commentingOn.side : undefined,
    );
    setCommentingOn(null);
  }

  function handleCancelComment() {
    setCommentingOn(null);
    setDragging(null);
  }

  function handlePostHighlightAsComment(h: Highlight) {
    if (!onCreateComment) return;
    const body = `**[AI ${h.severity.toUpperCase()}]** ${h.comment}`;
    const isRange = h.start_line !== h.end_line;
    onCreateComment(
      file.path,
      h.end_line,
      "RIGHT",
      body,
      isRange ? h.start_line : undefined,
      isRange ? "RIGHT" : undefined,
    );
  }

  useEffect(() => {
    if (!dragging) return;
    function onMouseUp() {
      handleLineMouseUp();
    }
    document.addEventListener("mouseup", onMouseUp);
    return () => document.removeEventListener("mouseup", onMouseUp);
  }, [dragging]);

  const lang = useMemo(() => detectLanguage(file.path), [file.path]);

  const { hunks, diffLines, useHunkView } = useMemo(() => {
    const hunkScores = file.hunk_scores ?? [];
    const hasUnifiedDiff = file.unified_diff && file.unified_diff.length > 0;

    // Use hunk-based view when we have scores and a parseable unified diff
    if (hasUnifiedDiff && hunkScores.length > 0) {
      const lines = parseDiffLines(file.unified_diff, lang, file.head_content, file.base_content);
      return {
        hunks: groupIntoHunks(lines, hunkScores),
        diffLines: lines,
        useHunkView: true,
      };
    }

    // Fallback: flat rendering (no hunk scores available)
    let lines: DiffLine[];
    if (file.diff_type === "added") {
      lines = buildFullFileLines(file.head_content, "add", lang);
    } else if (file.diff_type === "removed") {
      lines = buildFullFileLines(file.base_content, "remove", lang);
    } else {
      lines = parseDiffLines(file.unified_diff, lang, file.head_content, file.base_content);
      // Even without scores from AI, group into hunks for consistent rendering
      return {
        hunks: groupIntoHunks(lines, []),
        diffLines: lines,
        useHunkView: true,
      };
    }

    return {
      hunks: [],
      diffLines: lines,
      useHunkView: false,
    };
  }, [file]);

  // Whole-file diff (real adds/removes + all context), built lazily only when
  // the "view full file" toggle is on, then grouped into one hunk so it renders
  // through the normal split/unified views.
  const fullFileHunks = useMemo(
    () =>
      fullFile
        ? groupIntoHunks(buildFullFileDiffLines(file.head_content, file.unified_diff, lang), [])
        : [],
    [fullFile, file, lang]
  );
  // Offer "view full file" whenever we're showing a partial (hunk) diff and
  // have the new file content to expand into — this covers modified files and
  // files rendered as hunks regardless of their add/modify badge.
  const canViewFullFile = useHunkView && !!file.head_content;

  // A genuinely whole-new or whole-deleted file (no context lines, only one
  // side) can't be shown side-by-side, so it forces unified. Everything else —
  // including files mislabeled "added"/"removed" that are really modifications —
  // respects the split/unified toggle. Decide from content, not `diff_type`.
  const oneSidedFile = useMemo(() => {
    let ctx = false;
    let add = false;
    let rem = false;
    for (const l of diffLines) {
      if (l.type === "context") ctx = true;
      else if (l.type === "add") add = true;
      else if (l.type === "remove") rem = true;
    }
    return !ctx && add !== rem;
  }, [diffLines]);

  // Low hunks start collapsed when significance is shown; state resets on file change
  // via the key prop on DiffViewer (see App.tsx)
  const [collapsedHunks, setCollapsedHunks] = useState<Set<number>>(() => {
    if (!showHunkSignificance || hunks.length <= 1) return new Set<number>();
    return new Set(
      hunks.filter((h) => h.significance === "low").map((h) => h.index)
    );
  });

  const toggleHunk = (index: number) => {
    setCollapsedHunks((prev) => {
      const next = new Set(prev);
      if (next.has(index)) {
        next.delete(index);
      } else {
        next.add(index);
      }
      return next;
    });
  };

  const fileThreads = useMemo(
    () => (reviewThreads ?? []).filter((t) => t.path === file.path),
    [reviewThreads, file.path]
  );

  // AI notes split into active (shown in the diff + nav) and dismissed (restorable
  // from the summary). `highlights` is the active set everything else keys off.
  const allNotes = showAiNotes ? (file.highlights ?? []) : [];
  const keyFor = (h: Highlight) => highlightKey(file.path, h);
  const dismissed = dismissedHighlights ?? new Set<string>();
  const highlights = allNotes.filter((h) => !dismissed.has(keyFor(h)));
  const dismissedNotes = allNotes.filter((h) => dismissed.has(keyFor(h)));
  const [showDismissed, setShowDismissed] = useState(false);
  const isCritical =
    file.risk_level === "critical" || file.risk_level === "high";

  // Track original collapsed state so we can restore it when search ends
  const preSearchCollapsed = useRef<Set<number> | null>(null);
  const collapsedHunksRef = useRef(collapsedHunks);
  collapsedHunksRef.current = collapsedHunks;

  // Expand collapsed hunks that contain search matches; restore when search clears
  useEffect(() => {
    const hasSearch = searchQuery && searchMatches && searchMatches.length > 0;

    if (!hasSearch) {
      // Search ended — restore original collapsed state if we saved one
      if (preSearchCollapsed.current !== null) {
        setCollapsedHunks(preSearchCollapsed.current);
        preSearchCollapsed.current = null;
      }
      return;
    }

    // Save original collapsed state before we start expanding
    if (preSearchCollapsed.current === null) {
      preSearchCollapsed.current = new Set(collapsedHunksRef.current);
    }

    // Find which hunk indices contain search match line numbers
    const matchLineNums = new Set(searchMatches!.map((m) => m.lineNumber));
    const hunksToExpand = new Set<number>();
    for (const hunk of hunks) {
      for (const line of hunk.lines) {
        const ln = line.newLineNum ?? line.oldLineNum;
        if (ln != null && matchLineNums.has(ln)) {
          hunksToExpand.add(hunk.index);
          break;
        }
      }
    }

    if (hunksToExpand.size === 0) return;

    setCollapsedHunks((prev) => {
      const next = new Set(prev);
      for (const idx of hunksToExpand) {
        next.delete(idx);
      }
      if (next.size === prev.size) return prev;
      return next;
    });
  }, [searchQuery, searchMatches, hunks]);

  // Apply word-level search highlights into the DOM (marks all matches)
  useEffect(() => {
    const container = diffContentRef.current;
    if (!container) return;

    function clearMarks() {
      container!.querySelectorAll("mark.search-highlight, mark.search-highlight-current").forEach((el) => {
        const parent = el.parentNode;
        if (parent) {
          parent.replaceChild(document.createTextNode(el.textContent || ""), el);
          parent.normalize();
        }
      });
      container!.querySelectorAll(".search-match-line").forEach((el) => {
        el.classList.remove("search-match-line");
      });
    }

    clearMarks();

    if (!searchQuery || !searchMatches || searchMatches.length === 0) return;

    const query = searchQuery.toLowerCase();

    const pres = container.querySelectorAll<HTMLElement>(".line-content pre");
    for (const pre of pres) {
      const row = pre.closest("tr");
      if (!row) continue;

      const walker = document.createTreeWalker(pre, NodeFilter.SHOW_TEXT);
      const textNodes: Text[] = [];
      let node: Node | null;
      while ((node = walker.nextNode())) {
        textNodes.push(node as Text);
      }

      let hasMatch = false;
      let charOffset = 0;

      // Get the line number for this row (used as data attribute on marks)
      const lineNumCells = row.querySelectorAll<HTMLElement>(".line-num");
      let lineNum: number | null = null;
      for (const cell of lineNumCells) {
        const text = cell.textContent?.trim();
        if (text && /^\d+$/.test(text)) {
          lineNum = parseInt(text, 10);
          break;
        }
      }

      for (const textNode of textNodes) {
        const text = textNode.textContent || "";
        const textLower = text.toLowerCase();
        const parts: (string | { text: string; offset: number })[] = [];
        let lastEnd = 0;
        let searchStart = 0;

        while (true) {
          const idx = textLower.indexOf(query, searchStart);
          if (idx === -1) break;
          if (idx > lastEnd) parts.push(text.slice(lastEnd, idx));
          parts.push({ text: text.slice(idx, idx + query.length), offset: charOffset + idx });
          lastEnd = idx + query.length;
          searchStart = idx + 1;
          hasMatch = true;
        }

        charOffset += text.length;
        if (parts.length === 0) continue;
        if (lastEnd < text.length) parts.push(text.slice(lastEnd));

        const frag = document.createDocumentFragment();
        for (const part of parts) {
          if (typeof part === "string") {
            frag.appendChild(document.createTextNode(part));
          } else {
            const mark = document.createElement("mark");
            mark.className = "search-highlight";
            mark.dataset.line = String(lineNum ?? "");
            mark.dataset.offset = String(part.offset);
            mark.textContent = part.text;
            frag.appendChild(mark);
          }
        }
        textNode.parentNode!.replaceChild(frag, textNode);
      }

      if (hasMatch) row.classList.add("search-match-line");
    }

    return clearMarks;
  }, [searchMatches, searchQuery, file.path]);

  // Update which mark is the "current" one (lightweight — just swaps classes)
  useEffect(() => {
    const container = diffContentRef.current;
    if (!container) return;

    // Clear previous current mark
    const prev = container.querySelector("mark.search-highlight-current");
    if (prev) {
      prev.className = "search-highlight";
      prev.closest("tr")?.classList.remove("search-current-line");
    }

    if (!currentMatchInFile) return;

    // Find the mark matching the current search match by data attributes
    const marks = container.querySelectorAll<HTMLElement>("mark.search-highlight");
    for (const mark of marks) {
      if (
        mark.dataset.line === String(currentMatchInFile.lineNumber) &&
        mark.dataset.offset === String(currentMatchInFile.matchStart)
      ) {
        mark.className = "search-highlight-current";
        mark.closest("tr")?.classList.add("search-current-line");
        mark.scrollIntoView({ block: "center", behavior: "smooth" });
        return;
      }
    }

    // Fallback: scroll to the row by line number
    const allLineNums = container.querySelectorAll<HTMLElement>(".line-num");
    for (const el of allLineNums) {
      if (el.textContent?.trim() === String(currentMatchInFile.lineNumber)) {
        const row = el.closest("tr");
        if (row) {
          row.scrollIntoView({ block: "center", behavior: "smooth" });
          break;
        }
      }
    }
  }, [currentMatchInFile]);

  const highHunkCount = hunks.filter((h) => h.significance === "high").length;
  const collapsedCount = collapsedHunks.size;

  const collapseAll = () => {
    setCollapsedHunks(new Set(hunks.map((h) => h.index)));
  };

  const expandAll = () => {
    setCollapsedHunks(new Set());
  };

  // ── Tier 2: keyboard hunk/finding navigation + folding ────────────────────
  // Active notes only, sorted — so n/N skips dismissed ones. (Cheap; read on keypress.)
  const sortedFindings = highlights.slice().sort((a, b) => a.start_line - b.start_line);
  const findingIdxRef = useRef(-1);
  // Element id to scroll to + home the cursor onto after a re-render (finding nav,
  // fold-expand onto the first line, fold-collapse onto the hunk header).
  const [pendingScroll, setPendingScroll] = useState<string | null>(null);

  // Absolute scroll offset of an element within the diff-content scroll container.
  const offsetWithin = (el: HTMLElement, container: HTMLElement) =>
    el.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop;

  // The on-screen anchor for a hunk. Only a significance header OR a collapsed
  // placeholder carries id="hunk-N" — these are mutually exclusive on headerLine,
  // so there's never a duplicate id. Every other (expanded) hunk has rendered code
  // rows, so we fall back to its first line's #diff-line-N.
  const hunkAnchorEl = (index: number): HTMLElement | null => {
    const c = diffContentRef.current;
    if (!c) return null;
    const byId = c.querySelector<HTMLElement>(`#hunk-${index}`);
    if (byId) return byId;
    const hunk = hunks.find((h) => h.index === index);
    const first = hunk?.lines[0];
    const ln = first?.newLineNum ?? first?.oldLineNum;
    return ln != null ? c.querySelector<HTMLElement>(`#diff-line-${ln}`) : null;
  };

  const hunkOffsets = (): { index: number; top: number }[] => {
    const c = diffContentRef.current;
    if (!c) return [];
    return hunks
      .map((h) => {
        const el = hunkAnchorEl(h.index);
        return el ? { index: h.index, top: offsetWithin(el, c) } : null;
      })
      .filter((x): x is { index: number; top: number } => x !== null)
      .sort((a, b) => a.top - b.top);
  };

  const scrollDiffTo = (top: number) =>
    diffContentRef.current?.scrollTo({ top: Math.max(0, top), behavior: "smooth" });

  const goToAdjacentHunk = (dir: 1 | -1) => {
    const c = diffContentRef.current;
    if (!c) return;
    const offs = hunkOffsets();
    const target =
      dir > 0
        ? offs.find((o) => o.top > c.scrollTop + 4)
        : [...offs].reverse().find((o) => o.top < c.scrollTop - 4);
    if (target) scrollDiffTo(target.top);
  };
  const nextHunk = () => goToAdjacentHunk(1);
  const prevHunk = () => goToAdjacentHunk(-1);

  const foldHunk = () => {
    const c = diffContentRef.current;
    if (!c) return;
    // The hunk whose anchor sits at/above the viewport top is the "current" one.
    const cur = c.scrollTop + 4;
    let target: { index: number; top: number } | undefined;
    for (const o of hunkOffsets()) {
      if (o.top <= cur) target = o;
      else break;
    }
    if (target) toggleHunk(target.index);
  };

  const foldAll = () => {
    if (collapsedHunks.size > 0) expandAll();
    else collapseAll();
  };

  // ── Tier 3: line cursor ───────────────────────────────────────────────────
  // The cursor is a (line, side) pair tracked in refs (no re-renders) and painted
  // as a .cursor-line class on the matching [data-cl] row — same approach as the
  // search highlight. Navigable rows carry data-cl (line) + data-cs (side).
  // cs is the comment side, or "FOLD" when the cursor is on a collapsed-hunk row
  // (so `z` can expand it). cl is the line number, or the hunk index when "FOLD".
  type CursorPos = { cl: number; cs: "LEFT" | "RIGHT" | "FOLD" };
  const cursorRef = useRef<CursorPos | null>(null);
  const anchorRef = useRef<CursorPos | null>(null);
  const [replyTarget, setReplyTarget] = useState<{ threadId: string; commentId: string } | null>(null);
  const cursorThreadsByLine = useMemo(() => buildThreadsByLine(fileThreads), [fileThreads]);

  const navRows = (): HTMLElement[] => {
    const c = diffContentRef.current;
    return c ? Array.from(c.querySelectorAll<HTMLElement>("tr[data-cl]")) : [];
  };
  const keyOf = (r: HTMLElement): CursorPos => ({ cl: Number(r.dataset.cl), cs: r.dataset.cs as CursorPos["cs"] });
  const indexOfPos = (rows: HTMLElement[], pos: CursorPos | null) =>
    pos ? rows.findIndex((r) => Number(r.dataset.cl) === pos.cl && r.dataset.cs === pos.cs) : -1;

  const paintCursor = () => {
    const c = diffContentRef.current;
    if (!c) return;
    c.querySelectorAll(".cursor-line").forEach((e) => e.classList.remove("cursor-line"));
    c.querySelectorAll(".cursor-selected").forEach((e) => e.classList.remove("cursor-selected"));
    const cur = cursorRef.current;
    if (!cur) return;
    const rows = navRows();
    const ci = indexOfPos(rows, cur);
    if (ci < 0) return;
    rows[ci].classList.add("cursor-line");
    const anc = anchorRef.current;
    if (anc && anc.cs === cur.cs) {
      const ai = indexOfPos(rows, anc);
      if (ai >= 0) {
        const [lo, hi] = ai <= ci ? [ai, ci] : [ci, ai];
        for (let i = lo; i <= hi; i++) rows[i].classList.add("cursor-selected");
      }
    }
  };

  // Resolve the cursor's index, initializing it to the first row at/below the
  // viewport top when unset or scrolled off-screen.
  const cursorIndex = (rows: HTMLElement[]): number => {
    const found = indexOfPos(rows, cursorRef.current);
    if (found >= 0) return found;
    const c = diffContentRef.current;
    if (!c || rows.length === 0) return -1;
    let idx = 0;
    for (let i = 0; i < rows.length; i++) {
      idx = i;
      if (offsetWithin(rows[i], c) >= c.scrollTop) break;
    }
    cursorRef.current = keyOf(rows[idx]);
    return idx;
  };

  const moveCursorTo = (rows: HTMLElement[], i: number) => {
    const clamped = Math.max(0, Math.min(rows.length - 1, i));
    cursorRef.current = keyOf(rows[clamped]);
    paintCursor();
    rows[clamped].scrollIntoView({ block: "nearest" });
  };

  const cursorMove = (delta: number) => {
    const rows = navRows();
    if (rows.length) moveCursorTo(rows, cursorIndex(rows) + delta);
  };
  const cursorEdge = (edge: "top" | "bottom") => {
    const rows = navRows();
    if (rows.length) moveCursorTo(rows, edge === "top" ? 0 : rows.length - 1);
  };
  // Move the cursor ~frac of a viewport up/down, landing on the nearest row.
  const cursorPage = (dir: 1 | -1, frac: number) => {
    const c = diffContentRef.current;
    const rows = navRows();
    if (!c || !rows.length) return;
    const target = offsetWithin(rows[cursorIndex(rows)], c) + dir * c.clientHeight * frac;
    let best = 0;
    let bestDist = Infinity;
    rows.forEach((r, j) => {
      const d = Math.abs(offsetWithin(r, c) - target);
      if (d < bestDist) { bestDist = d; best = j; }
    });
    moveCursorTo(rows, best);
  };

  const toggleAnchor = () => {
    if (cursorIndex(navRows()) < 0) return;
    if (cursorRef.current?.cs === "FOLD") return; // selection only spans code lines
    anchorRef.current = anchorRef.current ? null : (cursorRef.current ? { ...cursorRef.current } : null);
    paintCursor();
  };

  const commentAtCursor = () => {
    const cur = cursorRef.current;
    if (!cur || cur.cs === "FOLD") return;
    const anc = anchorRef.current && anchorRef.current.cs === cur.cs ? anchorRef.current : null;
    setCommentingOn({
      startLine: anc ? Math.min(anc.cl, cur.cl) : cur.cl,
      endLine: anc ? Math.max(anc.cl, cur.cl) : cur.cl,
      side: cur.cs,
    });
    anchorRef.current = null;
    paintCursor();
  };

  const foldAtCursor = () => {
    const cur = cursorRef.current;
    if (cur && cur.cs === "FOLD") {
      // On a collapsed-hunk row: expand it and re-home the cursor onto its first
      // line (the pendingScroll effect picks up the now-rendered row).
      const hunk = hunks.find((h) => h.index === cur.cl);
      toggleHunk(cur.cl);
      const first = hunk?.lines[0];
      const ln = first ? (first.newLineNum ?? first.oldLineNum) : null;
      if (ln != null) setPendingScroll(`diff-line-${ln}`);
      return;
    }
    const hunk = cur
      ? hunks.find((h) => h.lines.some((l) => l.newLineNum === cur.cl || l.oldLineNum === cur.cl))
      : undefined;
    if (hunk) {
      toggleHunk(hunk.index);
      // If we just collapsed the hunk the cursor was in, its line is gone — land
      // the cursor on the hunk's now-collapsed header/placeholder row instead.
      if (!collapsedHunks.has(hunk.index)) setPendingScroll(`hunk-${hunk.index}`);
    } else {
      foldHunk();
    }
  };

  const threadAtCursor = (): ReviewThread | null => {
    const cur = cursorRef.current;
    if (!cur || cur.cs === "FOLD") return null;
    return cursorThreadsByLine.get(cur.cl)?.[0] ?? null;
  };
  const resolveAtCursor = () => {
    const t = threadAtCursor();
    if (t && onToggleResolved) onToggleResolved(t.id, !t.is_resolved);
  };
  const replyAtCursor = () => {
    const t = threadAtCursor();
    if (t && onReply && t.comments.length > 0) {
      setReplyTarget({ threadId: t.id, commentId: t.comments[t.comments.length - 1].id });
    }
  };

  const goToFinding = () => {
    const f = sortedFindings[findingIdxRef.current];
    if (!f) return;
    // Expand the containing hunk if collapsed, then move the cursor + scroll once
    // it has rendered. Findings live on the new (right) side.
    const hunk = hunks.find((h) =>
      h.lines.some((l) => l.newLineNum === f.start_line || l.oldLineNum === f.start_line),
    );
    if (hunk && collapsedHunks.has(hunk.index)) {
      setCollapsedHunks((prev) => {
        const n = new Set(prev);
        n.delete(hunk.index);
        return n;
      });
    }
    cursorRef.current = { cl: f.start_line, cs: "RIGHT" };
    anchorRef.current = null;
    setPendingScroll(`diff-line-${f.start_line}`);
  };

  const stepFinding = (dir: 1 | -1) => {
    const len = sortedFindings.length;
    if (len === 0) return;
    findingIdxRef.current = (findingIdxRef.current + dir + len) % len;
    goToFinding();
  };
  const nextFinding = () => stepFinding(1);
  const prevFinding = () => stepFinding(-1);

  // Scroll to + paint the cursor on the pending row once it has (re-)rendered.
  useEffect(() => {
    if (pendingScroll == null) return;
    const c = diffContentRef.current;
    if (c) {
      const el = c.querySelector<HTMLElement>(`#${pendingScroll}`);
      if (el) {
        el.scrollIntoView({ block: "center", behavior: "smooth" });
        el.classList.add("finding-flash");
        setTimeout(() => el.classList.remove("finding-flash"), 1200);
        // Home the cursor exactly onto the landed row (carries data-cl/data-cs).
        if (el.dataset.cl != null) cursorRef.current = { cl: Number(el.dataset.cl), cs: el.dataset.cs as CursorPos["cs"] };
      }
      paintCursor();
    }
    setPendingScroll(null);
  }, [pendingScroll]);

  // No dep array: the handle is only ever called imperatively (on a keypress),
  // so rebuilding it each render keeps it always-fresh for free.
  useImperativeHandle(ref, () => ({
    nextHunk, prevHunk, foldAll,
    cursorMove, cursorEdge, cursorPage, nextFinding, prevFinding, foldAtCursor,
    commentAtCursor, toggleAnchor, replyAtCursor, resolveAtCursor,
  }));

  return (
    <div className={`diff-viewer ${isCritical ? "diff-viewer-critical" : ""}`}>
      {replyTarget && (
        <KeyboardReplyOverlay
          onClose={() => setReplyTarget(null)}
          onSubmit={(body) => { onReply?.(replyTarget.threadId, replyTarget.commentId, body); setReplyTarget(null); }}
        />
      )}
      <div className="diff-header">
        <span className={`diff-badge diff-badge-${file.diff_type}`}>
          {file.diff_type.toUpperCase()}
        </span>
        <span className={`risk-badge risk-${file.risk_level}`}>
          {file.risk_level.toUpperCase()}
        </span>
        <span className="diff-file-path">{file.path}</span>
        <span className="diff-reason">{file.reason}</span>
        {highlights.length > 0 && (
          <span className="highlight-count">
            {highlights.length} AI {highlights.length === 1 ? "note" : "notes"}
          </span>
        )}
        {canViewFullFile && (
          <button
            className="hunk-toggle-all"
            onClick={() => setFullFile((v) => !v)}
            title={fullFile ? "Show only the changed hunks" : "Show the whole file with changes highlighted"}
          >
            {fullFile ? "View diff" : "View full file"}
          </button>
        )}
        {showHunkSignificance && highHunkCount > 0 && (
          <span className="hunk-high-summary">
            {highHunkCount} high-significance {highHunkCount === 1 ? "hunk" : "hunks"}
          </span>
        )}
        {!fullFile && collapsedCount > 0 && (
          <>
            <span className="hunk-collapse-summary">
              {collapsedCount} {collapsedCount === 1 ? "hunk" : "hunks"} collapsed
            </span>
            <button className="hunk-toggle-all" onClick={expandAll}>
              Expand All
            </button>
          </>
        )}
        {!fullFile && showHunkSignificance && collapsedCount === 0 && hunks.length > 1 && (
          <button className="hunk-toggle-all" onClick={collapseAll}>
            Collapse All
          </button>
        )}
      </div>
      {(highlights.length > 0 || dismissedNotes.length > 0) && (
        <div className="highlights-summary">
          {highlights.map((h, i) => (
            <div
              key={i}
              className={`highlights-summary-item highlight-${h.severity}`}
              style={{ cursor: "pointer" }}
              onClick={() => {
                const el = document.getElementById(`diff-line-${h.start_line}`);
                el?.scrollIntoView({ behavior: "smooth", block: "center" });
              }}
              title="Jump to code"
            >
              <span className="highlight-severity-badge">{h.severity.toUpperCase()}</span>
              <span className="highlight-lines">{formatLineRange(h.start_line, h.end_line)}</span>
              <span className="highlight-summary-text">{h.comment}</span>
              {onCreateComment && (
                <button
                  className="highlight-post-comment"
                  onClick={(e) => { e.stopPropagation(); handlePostHighlightAsComment(h); }}
                  title="Post this AI note as a review comment"
                >
                  Post as comment
                </button>
              )}
              {onToggleHighlightDismissed && (
                <button
                  className="highlight-dismiss"
                  onClick={(e) => { e.stopPropagation(); onToggleHighlightDismissed(keyFor(h)); }}
                  title="Dismiss this AI note"
                  aria-label="Dismiss AI note"
                >
                  ×
                </button>
              )}
            </div>
          ))}
          {dismissedNotes.length > 0 && (
            <button className="highlights-show-dismissed" onClick={() => setShowDismissed((v) => !v)}>
              {showDismissed ? "Hide" : "Show"} {dismissedNotes.length} dismissed
            </button>
          )}
          {showDismissed && dismissedNotes.map((h, i) => (
            <div
              key={`dismissed-${i}`}
              className={`highlights-summary-item highlight-${h.severity} highlight-dismissed`}
              style={{ cursor: "pointer" }}
              onClick={() => {
                const el = document.getElementById(`diff-line-${h.start_line}`);
                el?.scrollIntoView({ behavior: "smooth", block: "center" });
              }}
              title="Jump to code"
            >
              <span className="highlight-severity-badge">{h.severity.toUpperCase()}</span>
              <span className="highlight-lines">{formatLineRange(h.start_line, h.end_line)}</span>
              <span className="highlight-summary-text">{h.comment}</span>
              {onToggleHighlightDismissed && (
                <button
                  className="highlight-post-comment"
                  onClick={(e) => { e.stopPropagation(); onToggleHighlightDismissed(keyFor(h)); }}
                  title="Restore this AI note"
                >
                  Restore
                </button>
              )}
            </div>
          ))}
        </div>
      )}
      <div className="diff-content" ref={diffContentRef}>
        {fullFile || useHunkView ? (
          viewMode === "unified" || oneSidedFile ? (
            <UnifiedView
              hunks={fullFile ? fullFileHunks : hunks}
              highlights={highlights}
              collapsedHunks={fullFile ? NO_COLLAPSED : collapsedHunks}
              onToggleHunk={toggleHunk}
              showSignificance={showHunkSignificance}
              commentingOn={commentingOn}
              dragging={dragging}
              onLineMouseDown={onCreateComment ? handleLineMouseDown : undefined}
              onLineMouseEnter={handleLineMouseEnter}
              onLineMouseUp={handleLineMouseUp}
              onSubmitComment={handleSubmitComment}
              onCancelComment={handleCancelComment}
              reviewThreads={fileThreads}
              onEditComment={onEditComment}
              onReply={onReply}
              onToggleResolved={onToggleResolved}
              onToggleReaction={onToggleReaction}
              onPostHighlightAsComment={onCreateComment ? handlePostHighlightAsComment : undefined}
              lang={lang}
            />
          ) : (
            <SplitView
              hunks={fullFile ? fullFileHunks : hunks}
              highlights={highlights}
              collapsedHunks={fullFile ? NO_COLLAPSED : collapsedHunks}
              onToggleHunk={toggleHunk}
              showSignificance={showHunkSignificance}
              commentingOn={commentingOn}
              dragging={dragging}
              onLineMouseDown={onCreateComment ? handleLineMouseDown : undefined}
              onLineMouseEnter={handleLineMouseEnter}
              onLineMouseUp={handleLineMouseUp}
              onSubmitComment={handleSubmitComment}
              onCancelComment={handleCancelComment}
              reviewThreads={fileThreads}
              onEditComment={onEditComment}
              onReply={onReply}
              onToggleResolved={onToggleResolved}
              onToggleReaction={onToggleReaction}
              onPostHighlightAsComment={onCreateComment ? handlePostHighlightAsComment : undefined}
              lang={lang}
            />
          )
        ) : (
          <table className="diff-table unified">
            <colgroup>
              <col style={{ width: 50 }} />
              <col style={{ width: 50 }} />
              <col style={{ width: 20 }} />
              <col />
            </colgroup>
            <tbody>
              <UnifiedHunkLines lines={diffLines} highlights={highlights} commentingOn={commentingOn} dragging={dragging} onLineMouseDown={onCreateComment ? handleLineMouseDown : undefined} onLineMouseEnter={handleLineMouseEnter} onLineMouseUp={handleLineMouseUp} onSubmitComment={handleSubmitComment} onCancelComment={handleCancelComment} reviewThreads={fileThreads} onEditComment={onEditComment} onReply={onReply} onToggleResolved={onToggleResolved} onToggleReaction={onToggleReaction} onPostHighlightAsComment={onCreateComment ? handlePostHighlightAsComment : undefined} lang={lang} />
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
});

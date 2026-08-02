import { useMemo } from "react";
import hljs from "highlight.js";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import type { ChatAction } from "../types";

/** Render a fenced code block with highlight.js.
 *
 * SECURITY: the only innerHTML sink in the shared renderer, fed untrusted
 * input (PR bodies, AI answers). Safe today because hljs entity-escapes code
 * and the catch path runs escapeHtml — any change to this function must
 * preserve that; never interpolate unescaped input into the HTML. */
export function CodeBlock({ code, lang }: { code: string; lang?: string }) {
  const html = useMemo(() => {
    try {
      if (lang && hljs.getLanguage(lang)) {
        return hljs.highlight(code, { language: lang }).value;
      }
      return hljs.highlightAuto(code).value;
    } catch {
      return escapeHtml(code);
    }
  }, [code, lang]);
  return (
    <pre className="chat-code">
      <code dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  );
}

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

// Matches `path/to/file.ext` or `path/to/file.ext:123` — a bare file mention,
// optionally with a line number, as the AI tends to write them inline.
const FILE_REF_RE = /^([\w./-]+\.\w+)(?::(\d+))?$/;

/** Resolve a file mention against the in-scope manifest paths: an exact match,
 * or a unique suffix match (so `foo/bar.rs` matches `crates/foo/bar.rs`). */
function resolveFileRef(candidate: string, filePaths: string[]): string | null {
  if (filePaths.includes(candidate)) return candidate;
  const matches = filePaths.filter((p) => p.endsWith("/" + candidate));
  return matches.length === 1 ? matches[0] : null;
}

function parseFileRef(text: string, filePaths: string[]): { path: string; line?: number } | null {
  const m = text.match(FILE_REF_RE);
  if (!m) return null;
  const resolved = resolveFileRef(m[1], filePaths);
  if (!resolved) return null;
  return { path: resolved, line: m[2] ? Number(m[2]) : undefined };
}

/** External link, opened via the shell plugin (main-window contexts only —
 * everything that renders RichText lives in the main window). http(s) only. */
function ExternalLink({ label, url }: { label: string; url: string }) {
  const safe = /^https?:\/\//i.test(url);
  if (!safe) return <span>{label}</span>;
  return (
    <button type="button" className="rt-link" title={url} onClick={() => openUrl(url).catch(() => {})}>
      {label}
    </button>
  );
}

/** Italic runs (single *asterisk*, non-space-adjacent) within a plain span. */
function renderItalic(text: string, keyPrefix: string): React.ReactElement[] {
  return text.split(/(\*[^\s*][^*]*\*)/g).flatMap((ip, j) => {
    if (ip.length > 2 && ip.startsWith("*") && ip.endsWith("*") && !/\s$/.test(ip.slice(1, -1))) {
      return [<em key={`${keyPrefix}-i${j}`}>{unescapeMd(ip.slice(1, -1))}</em>];
    }
    return ip ? [<span key={`${keyPrefix}-i${j}`}>{unescapeMd(ip)}</span>] : [];
  });
}

/** Escaped `\\` and `\*` must be invisible to the emphasis delimiter regexes
 * (an escaped asterisk never opens emphasis), so they're sentinel-encoded
 * before splitting and decoded back at the text leaves in unescapeMd. `\\`
 * goes first so the backslash in `\\*` isn't misread as escaping the star. */
const ESC_BS = "\u0000";
const ESC_STAR = "\u0001";
function encodeEscapes(s: string): string {
  return s.replace(/\\\\/g, ESC_BS).replace(/\\\*/g, ESC_STAR);
}

/** GFM backslash escapes: \` \* \_ etc. render the punctuation literally.
 * Applied only at plain-text leaves — never inside code spans. `\\` and `\*`
 * arrive sentinel-encoded (see encodeEscapes). */
function unescapeMd(s: string): string {
  return s
    .replace(/\\([`_{}[\]()#+\-.!<>~|])/g, "$1")
    .replace(/\u0000/g, "\\")
    .replace(/\u0001/g, "*");
}

/** Bold and ***bold-italic*** (then italic) runs within a plain-text span. */
function renderBold(text: string, keyPrefix: string): React.ReactNode[] {
  return encodeEscapes(text).split(/(\*\*\*[^*]+\*\*\*|\*\*[^*]+\*\*)/g).flatMap((bp, j) => {
    if (bp.startsWith("***") && bp.endsWith("***") && bp.length > 6) {
      return [<strong key={`${keyPrefix}-${j}`}><em>{unescapeMd(bp.slice(3, -3))}</em></strong>];
    }
    if (bp.startsWith("**") && bp.endsWith("**") && bp.length > 4) {
      return [<strong key={`${keyPrefix}-${j}`}>{unescapeMd(bp.slice(2, -2))}</strong>];
    }
    return bp ? renderItalic(bp, `${keyPrefix}-${j}`) : [];
  });
}

/** CommonMark-style inline code-span scan: a run of N backticks opens a span
 * closed only by the next run of exactly N backticks (so `` a`b `` works and
 * multi-backtick runs don't pair off with random singles). Unclosed runs stay
 * literal text. */
function splitCodeSpans(text: string): Array<{ code: string } | { text: string }> {
  const out: Array<{ code: string } | { text: string }> = [];
  let plain = "";
  let i = 0;
  while (i < text.length) {
    if (text[i] !== "`") { plain += text[i]; i++; continue; }
    let n = 0;
    while (text[i + n] === "`") n++;
    // Find a closing run of exactly n backticks.
    let close = -1;
    for (let j = i + n; j < text.length; j++) {
      if (text[j] !== "`") continue;
      let m = 0;
      while (text[j + m] === "`") m++;
      if (m === n) { close = j; break; }
      j += m - 1;
    }
    if (close === -1) {
      plain += "`".repeat(n);
      i += n;
      continue;
    }
    if (plain) { out.push({ text: plain }); plain = ""; }
    let code = text.slice(i + n, close);
    // CommonMark: strip one leading+trailing space when both present and the
    // content isn't all spaces (lets you write code that starts with `).
    if (code.length >= 2 && code.startsWith(" ") && code.endsWith(" ") && code.trim() !== "") {
      code = code.slice(1, -1);
    }
    out.push({ code });
    i = close + n;
  }
  if (plain) out.push({ text: plain });
  return out;
}

/** Render inline `code` spans (recognizing in-scope file:line mentions as
 * clickable links), [text](url) / ![alt](url) links, and **bold**. */
export function renderInline(text: string, filePaths: string[], onOpenFile?: (path: string, line?: number) => void): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  // Code spans first — nothing inside them is markdown.
  const parts = splitCodeSpans(text);
  parts.forEach((tok, i) => {
    if ("code" in tok) {
      const inner = tok.code;
      const ref = filePaths.length > 0 ? parseFileRef(inner, filePaths) : null;
      if (ref && onOpenFile) {
        nodes.push(
          <button
            key={i}
            type="button"
            className="chat-file-link"
            onClick={() => onOpenFile(ref.path, ref.line)}
            title={`Open ${ref.path}${ref.line ? `:${ref.line}` : ""}`}
          >
            {inner}
          </button>,
        );
      } else {
        nodes.push(<code key={i} className="chat-inline-code">{inner}</code>);
      }
      return;
    }
    // Then links (and image syntax, rendered as a labeled link — no remote
    // image loading), then bold in the remaining plain runs.
    const linkParts = tok.text.split(/(!?\[[^\]]*\]\([^\s)]+\))/g);
    linkParts.forEach((lp, k) => {
      const m = lp.match(/^(!?)\[([^\]]*)\]\(([^\s)]+)\)$/);
      if (m) {
        const label = m[1] ? `🖼 ${m[2] || "image"}` : m[2] || m[3];
        nodes.push(<ExternalLink key={`${i}-${k}`} label={label} url={m[3]} />);
        return;
      }
      nodes.push(...renderBold(lp, `${i}-${k}`));
    });
  });
  return nodes;
}

type Block =
  | { kind: "heading"; level: number; text: string }
  | { kind: "quote"; lines: string[] }
  | { kind: "list"; ordered: boolean; items: { text: string; task?: "done" | "todo" }[] }
  | { kind: "hr" }
  | { kind: "fence"; lang?: string; code: string; closed: boolean }
  | { kind: "para"; lines: string[] };

const HEADING_RE = /^(#{1,4})\s+(.*)$/;
const LIST_RE = /^\s*(?:([-*])|(\d+)[.)])\s+(.*)$/;
const TASK_RE = /^\[([ xX])\]\s+(.*)$/;

/** Line-oriented block parser for GitHub-flavored PR bodies / chat answers.
 * Deliberately small — headings, lists (incl. task items), quotes, rules,
 * paragraphs. Nested lists flatten; unknown constructs degrade to text. */
function parseBlocks(segment: string): Block[] {
  const lines = segment.split("\n");
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.trim() === "") { i++; continue; }
    // Fenced code: only a line STARTING with ``` opens a fence (GitHub
    // semantics) — inline ``` inside a sentence stays literal text.
    const f = line.match(/^```([\w+-]*)\s*$/);
    if (f) {
      const code: string[] = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i])) {
        code.push(lines[i]);
        i++;
      }
      // Whether we stopped on a closing fence line or ran off the end of the
      // text (streaming chat mid-block) — unterminated fences still render as
      // code (or, for marrow-action, a pending pill; see RichText below).
      const closed = i < lines.length;
      i++; // consume the closing fence (a no-op past the end).
      blocks.push({ kind: "fence", lang: f[1] || undefined, code: code.join("\n"), closed });
      continue;
    }
    const h = line.match(HEADING_RE);
    if (h) { blocks.push({ kind: "heading", level: h[1].length, text: h[2] }); i++; continue; }
    if (/^\s*(-{3,}|\*{3,})\s*$/.test(line)) { blocks.push({ kind: "hr" }); i++; continue; }
    if (line.startsWith(">")) {
      const q: string[] = [];
      while (i < lines.length && lines[i].startsWith(">")) {
        q.push(lines[i].replace(/^>\s?/, ""));
        i++;
      }
      blocks.push({ kind: "quote", lines: q });
      continue;
    }
    const l = line.match(LIST_RE);
    if (l) {
      const ordered = !!l[2];
      const items: { text: string; task?: "done" | "todo" }[] = [];
      while (i < lines.length) {
        const im = lines[i].match(LIST_RE);
        if (!im || !!im[2] !== ordered) break;
        let text = im[3];
        let task: "done" | "todo" | undefined;
        const t = text.match(TASK_RE);
        if (t) { task = t[1].trim() ? "done" : "todo"; text = t[2]; }
        items.push({ text, task });
        i++;
        // Continuation lines (indented, not a new item) fold into the item.
        while (i < lines.length && lines[i].trim() !== "" && !LIST_RE.test(lines[i]) && /^\s{2,}/.test(lines[i])) {
          items[items.length - 1].text += " " + lines[i].trim();
          i++;
        }
      }
      blocks.push({ kind: "list", ordered, items });
      continue;
    }
    const p: string[] = [];
    while (i < lines.length && lines[i].trim() !== "" && !HEADING_RE.test(lines[i]) && !LIST_RE.test(lines[i]) && !lines[i].startsWith(">") && !/^```/.test(lines[i])) {
      p.push(lines[i]);
      i++;
    }
    blocks.push({ kind: "para", lines: p });
  }
  return blocks;
}

/** Runtime shape check for a parsed ```marrow-action payload — mirrors the
 * schemas documented in `CHAT_UI_ACTIONS` (crates/core/src/chat.rs). Anything
 * that doesn't match a known action renders as a plain code block instead of
 * a chip, same as unparseable JSON. */
function isChatAction(x: unknown): x is ChatAction {
  if (!x || typeof x !== "object") return false;
  const a = x as Record<string, unknown>;
  switch (a.action) {
    case "open_file":
      return typeof a.path === "string" && (a.line === undefined || typeof a.line === "number");
    case "open_overview":
    case "next_file":
    case "prev_file":
      return true;
    case "open_commit":
      return typeof a.sha === "string";
    case "set_hunk_filter":
      return a.filter === "all" || a.filter === "high" || a.filter === "medium";
    case "set_view_mode":
      return a.mode === "split" || a.mode === "unified";
    case "show_comments":
      return typeof a.open === "boolean";
    default:
      return false;
  }
}

/** Extract every CLOSED ```marrow-action fence from `text`, in appearance
 * order — reusing `parseBlocks` so this always segments text identically to
 * RichText's own rendering (no separate regex to drift out of sync). This is
 * the single source of truth for `blockIndex` assignment, shared by
 * RichText's chip rendering and App's streaming auto-execution effect.
 * Tolerant JSON parse: `action` is null when the block's content isn't valid
 * JSON or isn't a recognized action — callers fall back to raw-code / no-op. */
export function parseActionFences(text: string): { json: string; action: ChatAction | null }[] {
  const cleaned = text.replace(/<!--[\s\S]*?-->/g, "");
  const out: { json: string; action: ChatAction | null }[] = [];
  for (const b of parseBlocks(cleaned)) {
    if (b.kind !== "fence" || b.lang !== "marrow-action" || !b.closed) continue;
    const raw = b.code.trim();
    let action: ChatAction | null = null;
    try {
      const parsed = JSON.parse(raw);
      if (isChatAction(parsed)) action = parsed;
    } catch {
      // Unparseable — action stays null, block renders/counts as a miss.
    }
    out.push({ json: raw, action });
  }
  return out;
}

/** Same as `parseActionFences`, but first splits on `[[thought:N]]` dividers
 * exactly like ChatMarkdown does before handing text to RichText — so a
 * `marrow-action` fence numbers identically whether it's read here (App's
 * streaming auto-exec effect, scanning the raw message) or during rendering
 * (ChatMarkdown, which renders each split part through its own RichText
 * call). Splitting first means a divider landing inside a fence corrupts
 * that fence's parse the same way on both sides — instead of the two
 * disagreeing about where blocks start, which would misalign a status key
 * from its chip. */
export function parseChatActionFences(text: string): { json: string; action: ChatAction | null }[] {
  return text.split(/\[\[thought:\d+\]\]/g).flatMap(parseActionFences);
}

/** Human label for a chat-action chip. */
function actionChipLabel(a: ChatAction): string {
  switch (a.action) {
    case "open_file": {
      const base = a.path.split("/").pop() || a.path;
      return `Open ${base}${a.line ? `:${a.line}` : ""}`;
    }
    case "open_overview":
      return "Back to overview";
    case "next_file":
      return "Next file";
    case "prev_file":
      return "Previous file";
    case "open_commit":
      return `Open commit ${a.sha.slice(0, 7)}`;
    case "set_hunk_filter":
      return `Filter hunks: ${a.filter}`;
    case "set_view_mode":
      return `${a.mode} view`;
    case "show_comments":
      return a.open ? "Show comments" : "Hide comments";
  }
}

/** A clickable chip rendered in place of a completed ```marrow-action fence.
 * Neutral/clickable when `status` is absent (not yet run, or a restored
 * history message); ✓/✗ once it has been executed. `blockIndex` is this
 * fence's position among the CLOSED marrow-action fences in the containing
 * message (see `parseActionFences`) — passed back on click so the caller can
 * record the status under the same key it looks status up with. */
function ActionChip({
  action,
  blockIndex,
  status,
  onRunAction,
}: {
  action: ChatAction;
  blockIndex: number;
  status?: "done" | "failed";
  onRunAction?: (a: ChatAction, blockIndex: number) => void;
}) {
  const icon = status === "done" ? "✓" : status === "failed" ? "✗" : "→";
  return (
    <button
      type="button"
      className={`chat-action-chip${status ? ` chat-action-chip--${status}` : ""}`}
      onClick={() => onRunAction?.(action, blockIndex)}
    >
      <span className="chat-action-chip-icon" aria-hidden="true">{icon}</span>
      {actionChipLabel(action)}
    </button>
  );
}

/** Inline nodes for multiple source lines, preserving single newlines as
 * breaks (GitHub renders PR-body newlines hard, like comments). */
function renderLines(lines: string[], filePaths: string[], onOpenFile?: (path: string, line?: number) => void): React.ReactNode[] {
  return lines.flatMap((ln, i) => {
    const nodes = renderInline(ln, filePaths, onOpenFile);
    return i < lines.length - 1 ? [...nodes, <br key={`br-${i}`} />] : nodes;
  });
}

/** Minimal GitHub-flavored markdown: fenced code (highlighted), headings,
 * lists with task boxes, quotes, rules, links, inline code/bold, and
 * ```marrow-action fences (rendered as clickable chips, see ActionChip).
 * Deliberately small — the project has no Markdown dep; unknown syntax
 * degrades to text. */
export function RichText({
  content,
  filePaths = [],
  onOpenFile,
  onRunAction,
  actionStatuses,
  blockIndexOffset = 0,
}: {
  content: string;
  filePaths?: string[];
  onOpenFile?: (path: string, line?: number) => void;
  /** Chip click handler for a ```marrow-action block; `blockIndex` is its
   * position among this message's closed action fences (see ActionChip). */
  onRunAction?: (a: ChatAction, blockIndex: number) => void;
  /** Execution status for this message's action blocks, keyed by
   * `${blockIndex}:${JSON.stringify(action)}`. Absent entries render neutral. */
  actionStatuses?: Record<string, "done" | "failed">;
  /** Added to the blockIndex assigned to each action fence in this render —
   * lets a caller (ChatMarkdown) that splits one message into several
   * RichText calls (around `[[thought:N]]` dividers) keep indices contiguous
   * across the whole message instead of restarting at 0 per segment. */
  blockIndexOffset?: number;
}) {
  // PR templates are full of HTML comments — never render them.
  const cleaned = content.replace(/<!--[\s\S]*?-->/g, "");
  const actionFences = useMemo(() => parseActionFences(cleaned), [cleaned]);
  // Running pointer into actionFences, advanced once per closed marrow-action
  // fence encountered below — parseBlocks and parseActionFences segment the
  // same `cleaned` text identically, so this stays in lockstep.
  let actionPtr = 0;
  // Split on fenced code blocks, keeping the fences as delimiters.
  return (
    <>
      {(() => {
        return parseBlocks(cleaned).map((b, j) => {
          const key = `b-${j}`;
          switch (b.kind) {
            case "fence": {
              if (b.lang === "marrow-action") {
                if (!b.closed) {
                  return (
                    <span key={key} className="chat-action-chip chat-action-chip--pending">
                      action…
                    </span>
                  );
                }
                const localIndex = actionPtr++;
                const blockIndex = blockIndexOffset + localIndex;
                const entry = actionFences[localIndex];
                if (entry?.action) {
                  const statusKey = `${blockIndex}:${JSON.stringify(entry.action)}`;
                  return (
                    <ActionChip
                      key={key}
                      action={entry.action}
                      blockIndex={blockIndex}
                      status={actionStatuses?.[statusKey]}
                      onRunAction={onRunAction}
                    />
                  );
                }
                // Unparseable / unrecognized — fall back to a plain code block.
              }
              return <CodeBlock key={key} lang={b.lang} code={b.code} />;
            }
            case "heading":
              return <div key={key} className={`rt-h rt-h--${b.level}`}>{renderInline(b.text, filePaths, onOpenFile)}</div>;
            case "hr":
              return <hr key={key} className="rt-hr" />;
            case "quote":
              return <blockquote key={key} className="rt-quote">{renderLines(b.lines, filePaths, onOpenFile)}</blockquote>;
            case "list": {
              const Tag = b.ordered ? "ol" : "ul";
              return (
                <Tag key={key} className="rt-list">
                  {b.items.map((it, k) => (
                    <li key={k} className={it.task ? "rt-task" : undefined}>
                      {it.task && (
                        <span className={`rt-check rt-check--${it.task}`} aria-hidden>
                          {it.task === "done" ? "☑" : "☐"}
                        </span>
                      )}
                      {renderInline(it.text, filePaths, onOpenFile)}
                    </li>
                  ))}
                </Tag>
              );
            }
            case "para":
              return (
                <p key={key} className="chat-para">
                  {renderLines(b.lines, filePaths, onOpenFile)}
                </p>
              );
          }
        });
      })()}
    </>
  );
}

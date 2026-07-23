import { useMemo } from "react";
import hljs from "highlight.js";
import { open as openUrl } from "@tauri-apps/plugin-shell";

/** Render a fenced code block with highlight.js. */
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

/** Bold runs within a plain-text span. */
function renderBold(text: string, keyPrefix: string): React.ReactNode[] {
  return text.split(/(\*\*[^*]+\*\*)/g).flatMap((bp, j) => {
    if (bp.startsWith("**") && bp.endsWith("**") && bp.length > 4) {
      return [<strong key={`${keyPrefix}-${j}`}>{bp.slice(2, -2)}</strong>];
    }
    return bp ? [<span key={`${keyPrefix}-${j}`}>{bp}</span>] : [];
  });
}

/** Render inline `code` spans (recognizing in-scope file:line mentions as
 * clickable links), [text](url) / ![alt](url) links, and **bold**. */
export function renderInline(text: string, filePaths: string[], onOpenFile?: (path: string, line?: number) => void): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  // Code spans first — nothing inside them is markdown.
  const parts = text.split(/(`[^`]+`)/g);
  parts.forEach((part, i) => {
    if (part.startsWith("`") && part.endsWith("`") && part.length > 1) {
      const inner = part.slice(1, -1);
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
    const linkParts = part.split(/(!?\[[^\]]*\]\([^\s)]+\))/g);
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
    while (i < lines.length && lines[i].trim() !== "" && !HEADING_RE.test(lines[i]) && !LIST_RE.test(lines[i]) && !lines[i].startsWith(">")) {
      p.push(lines[i]);
      i++;
    }
    blocks.push({ kind: "para", lines: p });
  }
  return blocks;
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
 * lists with task boxes, quotes, rules, links, inline code/bold. Deliberately
 * small — the project has no Markdown dep; unknown syntax degrades to text. */
export function RichText({ content, filePaths = [], onOpenFile }: { content: string; filePaths?: string[]; onOpenFile?: (path: string, line?: number) => void }) {
  // PR templates are full of HTML comments — never render them.
  const cleaned = content.replace(/<!--[\s\S]*?-->/g, "");
  // Split on fenced code blocks, keeping the fences as delimiters.
  const segments = cleaned.split(/(```[\s\S]*?```)/g);
  return (
    <>
      {segments.map((seg, i) => {
        const fence = seg.match(/^```([\w+-]*)\n?([\s\S]*?)```$/);
        if (fence) {
          return <CodeBlock key={i} lang={fence[1] || undefined} code={fence[2].replace(/\n$/, "")} />;
        }
        return parseBlocks(seg).map((b, j) => {
          const key = `${i}-${j}`;
          switch (b.kind) {
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
      })}
    </>
  );
}

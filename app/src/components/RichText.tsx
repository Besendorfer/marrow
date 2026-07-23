import { useMemo } from "react";
import hljs from "highlight.js";

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

/** Render inline `code` spans (recognizing in-scope file:line mentions as
 * clickable links) and **bold** within a single text run. */
export function renderInline(text: string, filePaths: string[], onOpenFile?: (path: string, line?: number) => void): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  // Split on inline code first; bold is handled within non-code runs.
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
    const boldParts = part.split(/(\*\*[^*]+\*\*)/g);
    boldParts.forEach((bp, j) => {
      if (bp.startsWith("**") && bp.endsWith("**") && bp.length > 2) {
        nodes.push(<strong key={`${i}-${j}`}>{bp.slice(2, -2)}</strong>);
      } else if (bp) {
        nodes.push(<span key={`${i}-${j}`}>{bp}</span>);
      }
    });
  });
  return nodes;
}

/** Fenced ```code``` blocks (highlighted with highlight.js) plus paragraphs with
 * inline code and bold. Deliberately small — the project has no Markdown dep. */
export function RichText({ content, filePaths = [], onOpenFile }: { content: string; filePaths?: string[]; onOpenFile?: (path: string, line?: number) => void }) {
  // Split on fenced code blocks, keeping the fences as delimiters.
  const segments = content.split(/(```[\s\S]*?```)/g);
  return (
    <>
      {segments.map((seg, i) => {
        const fence = seg.match(/^```([\w+-]*)\n?([\s\S]*?)```$/);
        if (fence) {
          return <CodeBlock key={i} lang={fence[1] || undefined} code={fence[2].replace(/\n$/, "")} />;
        }
        return seg
          .split(/\n\n+/)
          .filter((p) => p.trim().length > 0)
          .map((para, j) => (
            <p key={`${i}-${j}`} className="chat-para">
              {renderInline(para, filePaths, onOpenFile)}
            </p>
          ));
      })}
    </>
  );
}

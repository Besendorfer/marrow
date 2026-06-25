import { useEffect, useMemo, useRef, useState } from "react";
import hljs from "highlight.js";
import type { ChatState } from "../types";

interface ChatPanelProps {
  chat: ChatState;
  /** Path of the file currently in focus (drives the default grounding scope). */
  selectedFilePath: string | null;
  onSend: (message: string) => void;
  onStop: () => void;
  onClose: () => void;
  onClear: () => void;
  onToggleWholePr: (value: boolean) => void;
}

/** Render a fenced code block with highlight.js. */
function CodeBlock({ code, lang }: { code: string; lang?: string }) {
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

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** Render inline `code` spans and **bold** within a single text run. */
function renderInline(text: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  // Split on inline code first; bold is handled within non-code runs.
  const parts = text.split(/(`[^`]+`)/g);
  parts.forEach((part, i) => {
    if (part.startsWith("`") && part.endsWith("`") && part.length > 1) {
      nodes.push(<code key={i} className="chat-inline-code">{part.slice(1, -1)}</code>);
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

/** A dim divider marking a gap where the AI used tools / thought between
 * answer segments. Backed by a `[[thought:<secs>]]` marker in the content. */
function ThoughtDivider({ seconds }: { seconds: number }) {
  return (
    <div className="chat-thought" aria-label={`Thought for ${seconds} seconds`}>
      <span className="chat-thought-rule" />
      <span className="chat-thought-label">Thought for {seconds}s</span>
      <span className="chat-thought-rule" />
    </div>
  );
}

/** Fenced ```code``` blocks (highlighted with highlight.js) plus paragraphs with
 * inline code and bold. Deliberately small — the project has no Markdown dep. */
function RichText({ content }: { content: string }) {
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
              {renderInline(para)}
            </p>
          ));
      })}
    </>
  );
}

/**
 * Renders assistant content: splits out `[[thought:<secs>]]` markers (rendered as
 * dim "Thought for Xs" dividers) and renders the text spans between them as
 * minimal Markdown.
 */
function ChatMarkdown({ content }: { content: string }) {
  // Capturing split → [text, secs, text, secs, text, …].
  const parts = content.split(/\[\[thought:(\d+)\]\]/g);
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <ThoughtDivider key={i} seconds={Number(part)} />
        ) : part ? (
          <RichText key={i} content={part} />
        ) : null,
      )}
    </>
  );
}

export function ChatPanel({
  chat,
  selectedFilePath,
  onSend,
  onStop,
  onClose,
  onClear,
  onToggleWholePr,
}: ChatPanelProps) {
  const [draft, setDraft] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const streaming = chat.status === "streaming";

  // Keep the transcript pinned to the bottom as content streams in.
  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [chat.messages, chat.streamingText]);

  function submit() {
    const text = draft.trim();
    if (!text || streaming) return;
    onSend(text);
    setDraft("");
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  const scopeLabel = chat.includeWholePr
    ? "Whole PR"
    : selectedFilePath
      ? selectedFilePath.split("/").pop()
      : "No file selected";

  return (
    <div className="chat-panel">
      <div className="chat-header">
        <span className="chat-title">Ask about this change</span>
        <div className="chat-header-actions">
          <button className="chat-icon-btn" onClick={onClear} title="Clear conversation" disabled={chat.messages.length === 0 && !streaming}>
            Clear
          </button>
          <button className="chat-icon-btn" onClick={onClose} title="Close chat (closes the panel)">
            ✕
          </button>
        </div>
      </div>

      <div className="chat-scope">
        <label className="chat-scope-toggle" title="Ground answers in every changed file instead of just the selected one">
          <input
            type="checkbox"
            checked={chat.includeWholePr}
            onChange={(e) => onToggleWholePr(e.target.checked)}
          />
          Include whole PR
        </label>
        <span className="chat-scope-indicator" title="Files in scope for grounding">
          Scope: {scopeLabel}
        </span>
      </div>

      <div className="chat-messages" ref={listRef}>
        {chat.messages.length === 0 && !streaming && (
          <div className="chat-empty">
            <p>Ask a question about this diff — for example:</p>
            <ul>
              <li>What's the blast radius of this change?</li>
              <li>Is the null/empty case handled?</li>
              <li>What calls this now?</li>
            </ul>
          </div>
        )}
        {chat.messages.map((m, i) => (
          <div key={i} className={`chat-msg chat-msg-${m.role}`}>
            <div className="chat-msg-role">{m.role === "user" ? "You" : "AI"}</div>
            <div className="chat-msg-body">
              {m.role === "assistant" ? <ChatMarkdown content={m.content} /> : <span className="chat-user-text">{m.content}</span>}
            </div>
          </div>
        ))}
        {streaming && (
          <div className="chat-msg chat-msg-assistant">
            <div className="chat-msg-role">AI</div>
            <div className="chat-msg-body">
              {chat.streamingText && <ChatMarkdown content={chat.streamingText} />}
              {chat.streamingStatus ? (
                <span className="chat-working"><span className="chat-working-spinner" aria-hidden="true">↻</span> {chat.streamingStatus}</span>
              ) : chat.streamingText ? (
                <span className="chat-caret" aria-hidden="true" />
              ) : (
                <span className="chat-thinking">Thinking…</span>
              )}
            </div>
          </div>
        )}
        {chat.error && <div className="chat-error" role="alert">{chat.error}</div>}
      </div>

      <div className="chat-input-row">
        <textarea
          ref={inputRef}
          className="chat-input"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={streaming ? "Generating answer…" : "Ask about this change…  (Enter to send, Shift+Enter for newline)"}
          rows={3}
          disabled={streaming}
        />
        {streaming ? (
          <button className="chat-send-btn chat-stop-btn" onClick={onStop}>
            Stop
          </button>
        ) : (
          <button className="chat-send-btn" onClick={submit} disabled={!draft.trim()}>
            Send
          </button>
        )}
      </div>
    </div>
  );
}

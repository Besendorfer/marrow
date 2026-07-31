import { useEffect, useRef, useState } from "react";
import type { ChatState } from "../types";
import { RichText } from "./RichText";

interface ChatPanelProps {
  chat: ChatState;
  /** Path of the file currently in focus (drives the default grounding scope). */
  selectedFilePath: string | null;
  /** Manifest file paths — used to recognize in-scope file mentions in answers. */
  filePaths: string[];
  onSend: (message: string) => void;
  onStop: () => void;
  onClose: () => void;
  onClear: () => void;
  onToggleWholePr: (value: boolean) => void;
  /** Jump to a file (and, when supported, a line) mentioned in an answer. */
  onOpenFile?: (path: string, line?: number) => void;
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

/**
 * Renders assistant content: splits out `[[thought:<secs>]]` markers (rendered as
 * dim "Thought for Xs" dividers) and renders the text spans between them as
 * minimal Markdown.
 */
function ChatMarkdown({ content, filePaths, onOpenFile }: { content: string; filePaths: string[]; onOpenFile?: (path: string, line?: number) => void }) {
  // Capturing split → [text, secs, text, secs, text, …].
  const parts = content.split(/\[\[thought:(\d+)\]\]/g);
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <ThoughtDivider key={i} seconds={Number(part)} />
        ) : part ? (
          <RichText key={i} content={part} filePaths={filePaths} onOpenFile={onOpenFile} />
        ) : null,
      )}
    </>
  );
}

export function ChatPanel({
  chat,
  selectedFilePath,
  filePaths,
  onSend,
  onStop,
  onClose,
  onClear,
  onToggleWholePr,
  onOpenFile,
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
      : "Whole PR";

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
        {selectedFilePath ? (
          <>
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
          </>
        ) : (
          <span className="chat-scope-indicator chat-scope-indicator--fixed" title="No file selected — grounding in every changed file">
            Scope: Whole PR
          </span>
        )}
      </div>

      <div className="chat-messages" ref={listRef}>
        {chat.messages.length === 0 && !streaming && (
          <div className="chat-empty">
            <p>Ask a question about this diff — for example:</p>
            <ul>
              <li>What's the riskiest part of this change?</li>
              <li>Walk me through this diff at a high level</li>
              <li>Are there edge cases these changes miss?</li>
            </ul>
          </div>
        )}
        {chat.messages.map((m, i) => (
          <div key={i} className={`chat-msg chat-msg-${m.role}`}>
            <div className="chat-msg-role">
              {m.role === "user" ? "You" : "AI"}
              {m.role === "user" && m.filePath && (
                <span className="chat-msg-scope">{m.filePath.split("/").pop()}</span>
              )}
            </div>
            <div className="chat-msg-body">
              {m.role === "assistant" ? <ChatMarkdown content={m.content} filePaths={filePaths} onOpenFile={onOpenFile} /> : <span className="chat-user-text">{m.content}</span>}
            </div>
          </div>
        ))}
        {streaming && (
          <div className="chat-msg chat-msg-assistant">
            <div className="chat-msg-role">AI</div>
            <div className="chat-msg-body">
              {chat.streamingText && <ChatMarkdown content={chat.streamingText} filePaths={filePaths} onOpenFile={onOpenFile} />}
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
          placeholder="Ask about this change…  (Enter to send, Shift+Enter for newline)"
          rows={3}
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

import type { ChatCard, ChatCardCell, ChatCardListItem } from "../types";

/** Row/column caps enforced at render time (the model is asked to stay under
 * these in the prompt, but nothing stops it from going over). */
const MAX_ROWS = 50;
const MAX_COLS = 8;

/** Runtime shape check for a parsed ```marrow-card payload — mirrors the
 * schemas documented in `CHAT_ANSWER_CARDS` (crates/core/src/chat.rs). Never
 * throws; anything that doesn't match falls back to a plain code block. Caps
 * (max rows/columns) are enforced at render, not here — a card with 200 rows
 * is still a valid card, just truncated on screen. */
// Kept in sync BY HAND with CHAT_ANSWER_CARDS (crates/core/src/chat.rs) and
// the ChatCard union (types.ts) — edit all three together.
function isChatCardCell(x: unknown): x is ChatCardCell {
  if (typeof x === "string") return true;
  if (!x || typeof x !== "object") return false;
  const c = x as Record<string, unknown>;
  return (
    typeof c.text === "string" &&
    (c.path === undefined || typeof c.path === "string") &&
    (c.line === undefined || typeof c.line === "number")
  );
}

function isChatCardListItem(x: unknown): x is ChatCardListItem {
  if (!x || typeof x !== "object") return false;
  const it = x as Record<string, unknown>;
  return (
    typeof it.text === "string" &&
    (it.detail === undefined || typeof it.detail === "string") &&
    (it.path === undefined || typeof it.path === "string") &&
    (it.line === undefined || typeof it.line === "number")
  );
}

export function isChatCard(x: unknown): x is ChatCard {
  if (!x || typeof x !== "object") return false;
  const c = x as Record<string, unknown>;
  if (c.title !== undefined && typeof c.title !== "string") return false;
  switch (c.type) {
    case "table":
      return (
        Array.isArray(c.columns) &&
        c.columns.every((col) => typeof col === "string") &&
        Array.isArray(c.rows) &&
        c.rows.every((row) => Array.isArray(row) && row.every(isChatCardCell))
      );
    case "list":
      return Array.isArray(c.items) && c.items.every(isChatCardListItem);
    default:
      return false;
  }
}

/** A single table cell: a jump button when it carries a `path`, plain text
 * otherwise. Mirrors the `chat-file-link` affordance RichText uses for inline
 * file mentions. */
function CardCell({ cell, onOpenFile }: { cell: ChatCardCell; onOpenFile?: (path: string, line?: number) => void }) {
  if (typeof cell === "string") return <>{cell}</>;
  if (cell.path && onOpenFile) {
    return (
      <button
        type="button"
        className="chat-file-link chat-card-cell-link"
        onClick={() => onOpenFile(cell.path!, cell.line)}
        title={`Open ${cell.path}${cell.line ? `:${cell.line}` : ""}`}
      >
        {cell.text}
      </button>
    );
  }
  return <>{cell.text}</>;
}

function TableCard({ card, onOpenFile }: { card: Extract<ChatCard, { type: "table" }>; onOpenFile?: (path: string, line?: number) => void }) {
  const columns = card.columns.slice(0, MAX_COLS);
  const rows = card.rows.slice(0, MAX_ROWS);
  const rowsCut = card.rows.length > MAX_ROWS;
  const colsCut = card.columns.length > MAX_COLS;
  // Say which cap actually fired — "first 50 rows" on a 9-column overflow lies.
  const truncNote = [rowsCut && `first ${MAX_ROWS} rows`, colsCut && `first ${MAX_COLS} columns`]
    .filter(Boolean)
    .join(", ");
  return (
    <div className="chat-card">
      {card.title && <div className="chat-card-title">{card.title}</div>}
      <div className="chat-card-table-wrap">
        <table className="chat-card-table">
          <thead>
            <tr>
              {columns.map((col, i) => (
                <th key={i}>{col}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, i) => (
              <tr key={i}>
                {row.slice(0, MAX_COLS).map((cell, j) => (
                  <td key={j}>
                    <CardCell cell={cell} onOpenFile={onOpenFile} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {truncNote && <div className="chat-card-truncated">Showing {truncNote}</div>}
    </div>
  );
}

function ListCard({ card, onOpenFile }: { card: Extract<ChatCard, { type: "list" }>; onOpenFile?: (path: string, line?: number) => void }) {
  const items = card.items.slice(0, MAX_ROWS);
  const truncated = card.items.length > MAX_ROWS;
  return (
    <div className="chat-card">
      {card.title && <div className="chat-card-title">{card.title}</div>}
      <ul className="chat-card-list">
        {items.map((it, i) => {
          const clickable = !!(it.path && onOpenFile);
          return (
            <li
              key={i}
              className={`chat-card-list-item${clickable ? " chat-card-list-item--clickable" : ""}`}
              onClick={clickable ? () => onOpenFile!(it.path!, it.line) : undefined}
            >
              <span className="chat-card-list-item-text">{it.text}</span>
              {it.detail && <span className="chat-card-list-item-detail">{it.detail}</span>}
            </li>
          );
        })}
      </ul>
      {truncated && <div className="chat-card-truncated">Showing first {MAX_ROWS} items</div>}
    </div>
  );
}

/** Renders a validated ```marrow-card payload as an interactive card — a
 * table or a list, never raw JSON. Pure rendering: no execution, no status,
 * no App.tsx state. `onOpenFile` is the same jump handler RichText already
 * threads through for inline file mentions and ```marrow-action chips. */
export function ChatCardView({ card, onOpenFile }: { card: ChatCard; onOpenFile?: (path: string, line?: number) => void }) {
  if (card.type === "table") return <TableCard card={card} onOpenFile={onOpenFile} />;
  return <ListCard card={card} onOpenFile={onOpenFile} />;
}

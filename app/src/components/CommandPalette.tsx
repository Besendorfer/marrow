import { useEffect, useMemo, useRef, useState } from "react";

export interface PaletteCommand {
  id: string;
  title: string;
  section: string;
  /** Display-only shortcut hint, e.g. "V" or "⌘R". */
  keys?: string;
  run: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  commands: PaletteCommand[];
}

// Rank: prefix match beats substring beats in-order subsequence.
function score(title: string, query: string): number {
  const t = title.toLowerCase();
  const q = query.toLowerCase();
  if (q === "") return 1;
  if (t.startsWith(q)) return 3;
  if (t.includes(q)) return 2;
  let i = 0;
  for (const ch of t) {
    if (ch === q[i]) i++;
    if (i === q.length) return 1;
  }
  return 0;
}

export function CommandPalette({ open, onClose, commands }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const matches = useMemo(() => {
    return commands
      .map((c) => ({ c, s: score(c.title, query.trim()) }))
      .filter((m) => m.s > 0)
      .sort((a, b) => b.s - a.s)
      .map((m) => m.c);
  }, [commands, query]);

  useEffect(() => {
    if (open) {
      setQuery("");
      setSelected(0);
      // Focus after the overlay renders.
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  useEffect(() => {
    setSelected(0);
  }, [query]);

  useEffect(() => {
    listRef.current
      ?.querySelector('[data-selected="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [selected, matches]);

  if (!open) return null;

  function runCommand(cmd: PaletteCommand) {
    onClose();
    cmd.run();
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((s) => Math.min(s + 1, matches.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const cmd = matches[selected];
      if (cmd) runCommand(cmd);
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  }

  let lastSection: string | null = null;

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <div className="palette-input-row">
          <span className="palette-icon" aria-hidden="true">⌕</span>
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Type a command…"
            aria-label="Command palette"
          />
        </div>
        <div className="palette-list" ref={listRef}>
          {matches.length === 0 && (
            <div className="palette-empty">No matching commands</div>
          )}
          {matches.map((cmd, i) => {
            const showSection = cmd.section !== lastSection;
            lastSection = cmd.section;
            return (
              <div key={cmd.id}>
                {showSection && (
                  <div className="palette-section">{cmd.section}</div>
                )}
                <button
                  className={`palette-row${i === selected ? " selected" : ""}`}
                  data-selected={i === selected}
                  onMouseEnter={() => setSelected(i)}
                  onClick={() => runCommand(cmd)}
                >
                  <span className="palette-title">{cmd.title}</span>
                  {cmd.keys && <kbd className="next-bar-key">{cmd.keys}</kbd>}
                </button>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

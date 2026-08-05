import { useEffect } from "react";

interface KeyboardHelpProps {
  onClose: () => void;
}

interface Binding {
  keys: string[];
  label: string;
}

interface Section {
  title: string;
  bindings: Binding[];
}

// Mirrors the implemented Tier 1 shortcuts in useKeyboardShortcuts.ts. Only list
// bindings that actually work — don't advertise TUI keys the GUI doesn't honor yet.
const SECTIONS: Section[] = [
  {
    title: "Views",
    bindings: [
      { keys: ["1"], label: "Overview" },
      { keys: ["2"], label: "Files" },
      { keys: ["3"], label: "Commits" },
      { keys: ["4"], label: "Checks" },
    ],
  },
  {
    title: "Navigation",
    bindings: [
      { keys: ["]"], label: "Next file" },
      { keys: ["["], label: "Previous file" },
      { keys: ["⌃Tab"], label: "Next tab" },
      { keys: ["⌃⇧Tab"], label: "Previous tab" },
      { keys: ["⌘t"], label: "New tab" },
      { keys: ["⌘w"], label: "Close tab" },
      { keys: ["j"], label: "Cursor down" },
      { keys: ["k"], label: "Cursor up" },
      { keys: ["g", "Home"], label: "Cursor to top" },
      { keys: ["G", "End"], label: "Cursor to bottom" },
      { keys: ["}"], label: "Next hunk" },
      { keys: ["{"], label: "Previous hunk" },
      { keys: ["n"], label: "Next AI note" },
      { keys: ["N"], label: "Previous AI note" },
      { keys: ["Space"], label: "Page down" },
      { keys: ["⇧Space"], label: "Page up" },
      { keys: ["⌃d"], label: "Half page down" },
      { keys: ["⌃u"], label: "Half page up" },
    ],
  },
  {
    title: "Review",
    bindings: [
      { keys: ["c"], label: "Comment on cursor line" },
      { keys: ["v"], label: "Start / extend selection" },
      { keys: ["r"], label: "Reply to thread at cursor" },
      { keys: ["x"], label: "Resolve / reopen thread" },
      { keys: ["R"], label: "Submit review (approve / changes / comment)" },
    ],
  },
  {
    title: "Actions",
    bindings: [
      { keys: ["z"], label: "Fold / unfold hunk at cursor" },
      { keys: ["Z"], label: "Fold / unfold all hunks" },
      { keys: ["V"], label: "Toggle file viewed" },
      { keys: ["T"], label: "Toggle comments panel" },
      { keys: ["⌃r", "F5"], label: "Refresh PR" },
      { keys: ["/"], label: "Search" },
      { keys: ["?"], label: "Toggle this help" },
      { keys: ["Esc"], label: "Close overlay" },
    ],
  },
];

export function KeyboardHelp({ onClose }: KeyboardHelpProps) {
  // Self-close on ?, q, or Esc — mirrors the TUI's help dismissal. The global
  // shortcut hook suppresses single keys while an overlay is open, so handle it here.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "?" || e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="settings-overlay" onClick={onClose}>
      <div className="kbd-help-modal" onClick={(e) => e.stopPropagation()}>
        <div className="settings-header">
          <h2>Keyboard shortcuts</h2>
          <button className="settings-close" onClick={onClose} aria-label="Close keyboard shortcuts">
            &times;
          </button>
        </div>
        <div className="kbd-help-grid">
          {SECTIONS.map((section) => (
            <div key={section.title} className="kbd-help-section">
              <h3 className="kbd-help-section-title">{section.title}</h3>
              {section.bindings.map((b) => (
                <div key={b.label} className="kbd-help-row">
                  <span className="kbd-help-keys">
                    {b.keys.map((k, i) => (
                      <span key={k}>
                        {i > 0 && <span className="kbd-help-or"> / </span>}
                        <kbd className="kbd-help-key">{k}</kbd>
                      </span>
                    ))}
                  </span>
                  <span className="kbd-help-label">{b.label}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

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
    title: "Navigation",
    bindings: [
      { keys: ["]"], label: "Next file" },
      { keys: ["["], label: "Previous file" },
      { keys: ["j"], label: "Scroll down" },
      { keys: ["k"], label: "Scroll up" },
      { keys: ["Space"], label: "Page down" },
      { keys: ["⇧Space"], label: "Page up" },
      { keys: ["⌃d"], label: "Half page down" },
      { keys: ["⌃u"], label: "Half page up" },
      { keys: ["g", "Home"], label: "Jump to top" },
      { keys: ["G", "End"], label: "Jump to bottom" },
    ],
  },
  {
    title: "Actions",
    bindings: [
      { keys: ["V"], label: "Toggle file viewed" },
      { keys: ["T"], label: "Toggle threads / diff" },
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

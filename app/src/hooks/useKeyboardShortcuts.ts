import { useEffect, useRef } from "react";

/**
 * Global keyboard shortcuts for the review GUI, ported from the CLI/TUI
 * keybindings (see crates/cli/src/tui.rs `on_key`). This is Tier 1: navigation
 * and the actions that map cleanly onto existing handlers. Movement keys
 * (j/k, ^d/^u, g/G, …) scroll the diff pane rather than driving a line cursor —
 * the GUI has no cursor concept, so faithful c/v/z/n parity is deferred.
 */
export interface ShortcutHandlers {
  onNextFile: () => void;
  onPrevFile: () => void;
  onToggleViewed: () => void;
  onToggleThreads: () => void;
  onRefresh: () => void;
  onOpenSearch: () => void;
  onToggleHelp: () => void;
  /** Close transient overlays (help). Search owns its own Esc. */
  onCloseOverlays: () => void;
  /** Cycle to the next / previous tab (Ctrl+Tab / Ctrl+Shift+Tab). */
  onNextTab: () => void;
  onPrevTab: () => void;
  /** Close the active tab (Ctrl+W on Win/Linux; Cmd+W is handled by the native menu on macOS). */
  onCloseTab: () => void;
  /** Open a new tab (Ctrl+T on Win/Linux; Cmd+T is handled by the native menu on macOS). */
  onNewTab: () => void;
  // Tier 2/3 — diff-internal navigation/folding (no-ops when no diff is shown).
  onNextHunk: () => void;
  onPrevHunk: () => void;
  onNextFinding: () => void;
  onPrevFinding: () => void;
  onFoldAll: () => void;
  // Tier 3 — line cursor. j/k/g/G drive the cursor instead of scrolling.
  onCursorDown: () => void;
  onCursorUp: () => void;
  onCursorTop: () => void;
  onCursorBottom: () => void;
  onCursorHalfDown: () => void;
  onCursorHalfUp: () => void;
  onCursorPageDown: () => void;
  onCursorPageUp: () => void;
  onFoldAtCursor: () => void;
  onComment: () => void;
  onToggleAnchor: () => void;
  onReviewPicker: () => void;
  onReply: () => void;
  onResolve: () => void;
  /** Toggle the conversational diff Q&A panel (Cmd/Ctrl+J). */
  onToggleChat: () => void;
}

export interface ShortcutOptions {
  /** False on the opener/loading tab — no diff to drive, so suppress everything. */
  enabled: boolean;
  /** A modal (help/search/settings/checks) is open — suppress single-key nav, but keep Esc. */
  overlayOpen: boolean;
}

function isEditable(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}

// ARIA roles whose widgets respond to Space/Enter — paging should defer to them.
const INTERACTIVE_ROLES = new Set([
  "button", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
  "menuitemradio", "option", "link",
]);

/** Elements where Space/Enter means "activate", so paging should defer to them. */
function isClickable(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  if (tag === "BUTTON" || tag === "A" || tag === "SUMMARY") return true;
  const role = el.getAttribute("role");
  return role !== null && INTERACTIVE_ROLES.has(role);
}

export function useKeyboardShortcuts(handlers: ShortcutHandlers, options: ShortcutOptions): void {
  // Keep the latest handlers/options in a ref so the listener is registered once
  // and never sees a stale closure as App re-renders.
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;
  const optionsRef = useRef(options);
  optionsRef.current = options;

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const h = handlersRef.current;
      const { enabled, overlayOpen } = optionsRef.current;

      // Windows/Linux tab control (macOS uses Cmd via the native menu). Ctrl+W closes
      // the active tab (when not typing), Ctrl+T opens a new tab, Ctrl+Q is a no-op.
      // Handled before the typing guard so Ctrl+Q never quits even from a focused field.
      if (e.ctrlKey && !e.metaKey && !e.altKey) {
        const k = e.key.toLowerCase();
        if (k === "q") { e.preventDefault(); return; }
        if (k === "t") { e.preventDefault(); h.onNewTab(); return; }
        if (k === "w") {
          e.preventDefault();
          if (!isEditable(e.target)) h.onCloseTab();
          return;
        }
      }

      // Cmd/Ctrl+J toggles the chat panel. Handled before the typing guard so it
      // also closes the panel while the chat input is focused. Gated on `enabled`
      // (no chat on the opener tab) but allowed with an overlay open.
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "j") {
        if (enabled) {
          h.onToggleChat();
          e.preventDefault();
        }
        return;
      }

      // Typing into a field never triggers shortcuts.
      if (isEditable(e.target)) return;

      // Esc is always allowed so overlays can be dismissed.
      if (e.key === "Escape") {
        h.onCloseOverlays();
        return;
      }

      // Ctrl+Tab / Ctrl+Shift+Tab cycle tabs — works regardless of `enabled` (so it
      // applies on opener tabs too), but not while an overlay is open. Ctrl, not Cmd:
      // Cmd+Tab is the macOS app switcher.
      if (e.key === "Tab" && e.ctrlKey && !e.metaKey && !e.altKey) {
        if (!overlayOpen) {
          if (e.shiftKey) h.onPrevTab(); else h.onNextTab();
          e.preventDefault();
        }
        return;
      }

      if (!enabled || overlayOpen) return;

      // Vim-style cursor paging is Ctrl-only (NOT Cmd): on macOS Cmd+R reloads the
      // webview, Cmd+D bookmarks, Cmd+U views source — leave those to the system.
      // ^f (full page down in the TUI) is omitted too; it collides with find, which
      // SearchBar's own Cmd/Ctrl+F listener owns. Space covers the gap.
      if (e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey) {
        switch (e.key.toLowerCase()) {
          case "d": h.onCursorHalfDown(); e.preventDefault(); return;
          case "u": h.onCursorHalfUp(); e.preventDefault(); return;
          case "b": h.onCursorPageUp(); e.preventDefault(); return;
          case "r": h.onRefresh(); e.preventDefault(); return;
        }
      }

      // Space / Shift+Space page the cursor (laptop-friendly PageDown/Up).
      // Skip when a clickable control is focused, where Space means "activate".
      if (e.key === " " && !isClickable(e.target)) {
        if (e.shiftKey) h.onCursorPageUp(); else h.onCursorPageDown();
        e.preventDefault();
        return;
      }

      // Everything past here is a plain (unmodified) keypress.
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      switch (e.key) {
        case "]": h.onNextFile(); break;
        case "[": h.onPrevFile(); break;
        case "V": h.onToggleViewed(); break;
        case "T": h.onToggleThreads(); break;
        case "F5": h.onRefresh(); e.preventDefault(); break;
        case "/": h.onOpenSearch(); e.preventDefault(); break;
        case "?": h.onToggleHelp(); break;
        // Tier 3: j/k/g/G drive the line cursor (the view follows it).
        case "j": h.onCursorDown(); break;
        case "k": h.onCursorUp(); break;
        case "g": case "Home": h.onCursorTop(); break;
        case "G": case "End": h.onCursorBottom(); break;
        case "}": h.onNextHunk(); break;
        case "{": h.onPrevHunk(); break;
        case "n": h.onNextFinding(); break;
        case "N": h.onPrevFinding(); break;
        case "z": h.onFoldAtCursor(); break;
        case "Z": h.onFoldAll(); break;
        case "c": h.onComment(); break;
        case "v": h.onToggleAnchor(); break;
        case "r": h.onReply(); break;
        case "R": h.onReviewPicker(); break;
        case "x": h.onResolve(); break;
        case "PageDown": h.onCursorPageDown(); e.preventDefault(); break;
        case "PageUp": h.onCursorPageUp(); e.preventDefault(); break;
        default: return;
      }
    }

    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);
}

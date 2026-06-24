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
}

export interface ShortcutOptions {
  /** False on the opener/loading tab — no diff to drive, so suppress everything. */
  enabled: boolean;
  /** A modal (help/search/settings/checks) is open — suppress single-key nav, but keep Esc. */
  overlayOpen: boolean;
  /** Returns the scrollable diff element. Defaults to the `.diff-content` pane. */
  getScrollEl?: () => HTMLElement | null;
}

/** ~3 text lines — a comfortable j/k nudge. */
const LINE_STEP = 48;

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
    function scrollEl(): HTMLElement | null {
      const get = optionsRef.current.getScrollEl;
      return get ? get() : (document.querySelector(".diff-content") as HTMLElement | null);
    }
    function scrollBy(delta: number) {
      scrollEl()?.scrollBy({ top: delta });
    }
    function scrollToEdge(edge: "top" | "bottom") {
      const el = scrollEl();
      if (!el) return;
      el.scrollTo({ top: edge === "top" ? 0 : el.scrollHeight });
    }
    function page(fraction: number): number {
      const el = scrollEl();
      return el ? el.clientHeight * fraction : 0;
    }

    function onKey(e: KeyboardEvent) {
      const h = handlersRef.current;
      const { enabled, overlayOpen } = optionsRef.current;

      // Typing into a field never triggers shortcuts.
      if (isEditable(e.target)) return;

      // Esc is always allowed so overlays can be dismissed.
      if (e.key === "Escape") {
        h.onCloseOverlays();
        return;
      }

      if (!enabled || overlayOpen) return;

      // Vim-style scroll/refresh combos are Ctrl-only (NOT Cmd): on macOS Cmd+R
      // reloads the webview, Cmd+D bookmarks, Cmd+U views source — leave those to
      // the system. ^f (full page down in the TUI) is omitted too; it collides with
      // find, which SearchBar's own Cmd/Ctrl+F listener owns. Space covers the gap.
      if (e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey) {
        switch (e.key.toLowerCase()) {
          case "d": scrollBy(page(0.5)); e.preventDefault(); return;
          case "u": scrollBy(-page(0.5)); e.preventDefault(); return;
          case "b": scrollBy(-page(0.9)); e.preventDefault(); return;
          case "r": h.onRefresh(); e.preventDefault(); return;
        }
      }

      // Space / Shift+Space page through the diff (the laptop-friendly PageDown/Up).
      // Skip when a clickable control is focused, where Space means "activate".
      if (e.key === " " && !isClickable(e.target)) {
        scrollBy(e.shiftKey ? -page(0.9) : page(0.9));
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
        case "j": scrollBy(LINE_STEP); break;
        case "k": scrollBy(-LINE_STEP); break;
        case "g": case "Home": scrollToEdge("top"); break;
        case "G": case "End": scrollToEdge("bottom"); break;
        case "PageDown": scrollBy(page(0.9)); e.preventDefault(); break;
        case "PageUp": scrollBy(-page(0.9)); e.preventDefault(); break;
        default: return;
      }
    }

    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);
}

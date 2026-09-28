// The shared modal dialog (issue #238 phase 7): role="dialog" + aria-modal,
// focus moves in on open and back to where it was on close, Tab stays inside,
// Esc and a backdrop click close. Surfaces supply their own classes.
import { useEffect, useRef, type KeyboardEvent, type ReactNode, type RefObject } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Where Tab goes from `current` among `count` focusables: wraps at either
 * end; from outside them (the panel itself, -1) it enters at the start, or
 * at the end with Shift. */
export function trapTarget(count: number, current: number, shift: boolean): number {
  if (count === 0) return -1;
  if (current < 0) return shift ? count - 1 : 0;
  if (shift) return current === 0 ? count - 1 : current - 1;
  return current === count - 1 ? 0 : current + 1;
}

export interface DialogProps {
  label: string;
  onClose: () => void;
  className: string;
  backdropClassName: string;
  children: ReactNode;
  /** The dialog element, for surfaces that move focus back to it later. */
  panelRef?: RefObject<HTMLDivElement | null>;
  /** Extra keys (e.g. ⌘↵ to submit); runs before the dialog's own. */
  onKeyDown?: (e: KeyboardEvent<HTMLDivElement>) => void;
}

export function Dialog({ label, onClose, className, backdropClassName, children, panelRef, onKeyDown }: DialogProps) {
  const ownRef = useRef<HTMLDivElement>(null);
  const ref = panelRef ?? ownRef;

  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    ref.current?.focus();
    return () => {
      // Back to whatever opened it, if that's still on screen.
      if (opener && opener.isConnected) opener.focus({ preventScroll: true });
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  function handleKey(e: KeyboardEvent<HTMLDivElement>) {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key === "Escape") {
      e.preventDefault();
      // Global Esc handlers (search, help) must not also run.
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === "Tab" && ref.current) {
      const items = Array.from(ref.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => el.offsetParent !== null);
      const next = trapTarget(items.length, items.indexOf(document.activeElement as HTMLElement), e.shiftKey);
      e.preventDefault();
      if (next >= 0) items[next].focus();
    }
  }

  return (
    <div className={backdropClassName} onMouseDown={onClose}>
      <div
        className={className}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
        ref={ref}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={handleKey}
      >
        {children}
      </div>
    </div>
  );
}

// A draggable, keyboard-operable divider between two panes (issue #238
// phase 7). It's a focusable ARIA separator: arrows nudge the width, Home/End
// jump to the bounds, double-click or Enter resets to the default.
import { useRef, type KeyboardEvent, type PointerEvent } from "react";
import type { PaneBounds } from "../hooks/usePaneWidth";

const STEP = 16;

export interface SplitterProps {
  /** What it resizes, for screen readers ("Review list width"). */
  label: string;
  width: number;
  bounds: PaneBounds;
  onChange: (width: number) => void;
  /** The pane sits to the splitter's right (a right dock), so dragging
   * left grows it. */
  paneOnRight?: boolean;
}

export function Splitter({ label, width, bounds, onChange, paneOnRight }: SplitterProps) {
  const drag = useRef<{ x: number; width: number } | null>(null);
  const sign = paneOnRight ? -1 : 1;

  function onPointerDown(e: PointerEvent<HTMLDivElement>) {
    if (e.button !== 0) return;
    e.preventDefault();
    drag.current = { x: e.clientX, width };
    e.currentTarget.setPointerCapture(e.pointerId);
    document.body.classList.add("is-resizing");
  }

  function onPointerMove(e: PointerEvent<HTMLDivElement>) {
    if (!drag.current) return;
    onChange(drag.current.width + sign * (e.clientX - drag.current.x));
  }

  function endDrag(e: PointerEvent<HTMLDivElement>) {
    if (!drag.current) return;
    drag.current = null;
    e.currentTarget.releasePointerCapture(e.pointerId);
    document.body.classList.remove("is-resizing");
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    let next: number | null = null;
    if (e.key === "ArrowLeft") next = width - sign * STEP;
    else if (e.key === "ArrowRight") next = width + sign * STEP;
    else if (e.key === "Home") next = bounds.min;
    else if (e.key === "End") next = bounds.max;
    else if (e.key === "Enter") next = bounds.initial;
    if (next == null) return;
    // Keep the diff's own arrow/Home/End handling out of it.
    e.preventDefault();
    e.stopPropagation();
    onChange(next);
  }

  return (
    <div
      className="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={width}
      aria-valuemin={bounds.min}
      aria-valuemax={bounds.max}
      tabIndex={0}
      title="Drag to resize · double-click to reset"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={() => onChange(bounds.initial)}
      onKeyDown={onKeyDown}
    />
  );
}

import { useEffect, useRef, useState } from "react";

type ReviewEvent = "APPROVE" | "REQUEST_CHANGES" | "COMMENT";

const VERBS: { key: string; event: ReviewEvent; label: string }[] = [
  { key: "a", event: "APPROVE", label: "Approve" },
  { key: "r", event: "REQUEST_CHANGES", label: "Request changes" },
  { key: "c", event: "COMMENT", label: "Comment" },
];

/**
 * Keyboard review picker (the `R` shortcut), mirroring the TUI's R → a/r/c → body
 * flow: pick a verb, type an optional/required body, submit with Cmd/Ctrl+Enter.
 */
export function ReviewPicker({ onSubmit, onClose }: {
  onSubmit: (event: ReviewEvent, body: string) => void;
  onClose: () => void;
}) {
  const [verb, setVerb] = useState<ReviewEvent | null>(null);
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);

  // Step 1: capture a / r / c to choose the verb (Esc cancels).
  useEffect(() => {
    if (verb) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") { e.preventDefault(); onClose(); return; }
      const v = VERBS.find((x) => x.key === e.key.toLowerCase());
      if (v) { e.preventDefault(); setVerb(v.event); }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [verb, onClose]);

  useEffect(() => { if (verb) ref.current?.focus(); }, [verb]);

  const submit = () => {
    if (submitting || !verb) return;
    // GitHub requires a body for COMMENT / REQUEST_CHANGES; APPROVE may be empty.
    if (verb !== "APPROVE" && !text.trim()) return;
    setSubmitting(true);
    onSubmit(verb, text.trim());
  };

  const verbLabel = VERBS.find((v) => v.event === verb)?.label;

  return (
    <div className="kbd-overlay-backdrop" onClick={onClose}>
      <div className="kbd-overlay" onClick={(e) => e.stopPropagation()}>
        <div className="kbd-overlay-title">Submit review{verbLabel ? `: ${verbLabel}` : ""}</div>
        {!verb ? (
          <>
            <div className="kbd-overlay-verbs">
              {VERBS.map((v) => (
                <button key={v.event} className="kbd-overlay-verb" onClick={() => setVerb(v.event)}>
                  {v.label} <kbd>{v.key}</kbd>
                </button>
              ))}
            </div>
            <div className="kbd-overlay-hint">Choose a verb (a / r / c), or Esc to cancel.</div>
          </>
        ) : (
          <>
            <textarea
              ref={ref}
              className="kbd-overlay-input"
              value={text}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") { e.preventDefault(); onClose(); }
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submit(); }
              }}
              placeholder={verb === "APPROVE" ? "Optional comment…" : "Leave a comment…"}
            />
            <div className="kbd-overlay-hint">
              {verb === "APPROVE" ? "Body optional. " : ""}⌘/Ctrl+Enter to submit · Esc to cancel
            </div>
          </>
        )}
      </div>
    </div>
  );
}

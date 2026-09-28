// Finish panel (issue #238 phase 5): wrap up a review in one place, per tab.
// A recap of the findings and your batched comments, CI as a status line
// (never a blocker), the verdict, an AI-drafted body, and submit — then a
// done state that offers the next PR in your queue.

import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { FinishDone, FinishDraft, PrChecksStatus, ReviewManifest, ReviewRequestItem, ReviewThread, Tab } from "../types";
import { buildFindings, type Finding } from "../review/findings";
import { Dialog } from "./Dialog";
import { attemptSubmit, ciStatus, defaultVerb, filesReviewed, mergeDraft, pendingComments, recapSummary, submitBlocker, type ReviewEvent } from "../review/finish";

const VERBS: { event: ReviewEvent; label: string; hint: string }[] = [
  { event: "APPROVE", label: "Approve", hint: "Ready to merge" },
  { event: "COMMENT", label: "Comment", hint: "Feedback without a verdict" },
  { event: "REQUEST_CHANGES", label: "Request changes", hint: "Must change before merge" },
];

const DONE_LABEL: Record<ReviewEvent, string> = {
  APPROVE: "Approved",
  COMMENT: "Review posted",
  REQUEST_CHANGES: "Changes requested",
};

const STATE_MARK: Record<Finding["state"], string> = { open: "○", checked: "✓", commented: "✎", dismissed: "–" };

export interface FinishPanelProps {
  tab: Tab & { manifest: ReviewManifest };
  checks: PrChecksStatus | null;
  viewerLogin: string | null;
  onClose: () => void;
  onDraftChange: (patch: Partial<FinishDraft>) => void;
  onDone: (done: FinishDone) => void;
  onDraftBody: (openDefects: Finding[]) => Promise<string>;
  onSubmit: (event: ReviewEvent, body: string) => Promise<void>;
  onJumpToFinding: (f: Finding) => void;
  onJumpToThread: (thread: ReviewThread) => void;
  onNextInQueue: () => Promise<ReviewRequestItem | null>;
  onOpenPr: (ref: string) => void;
  onBackToQueue: () => void;
}

function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

export function FinishPanel(props: FinishPanelProps) {
  const { tab, checks, viewerLogin } = props;
  const manifest = tab.manifest;
  const panelRef = useRef<HTMLDivElement>(null);
  const threads = tab.commentThreads.status === "loaded" ? tab.commentThreads.threads : undefined;
  const isMerged = tab.myReviewState?.is_merged ?? false;

  const findings = useMemo(
    () =>
      buildFindings(manifest, {
        dismissed: tab.dismissedHighlights,
        checked: tab.checkedFindings,
        checks,
        resolvedSpecKeys: tab.resolvedSpecKeys,
        threads,
        viewerLogin,
      }).findings,
    [manifest, tab.dismissedHighlights, tab.checkedFindings, checks, tab.resolvedSpecKeys, threads, viewerLogin],
  );
  const toFix = findings.filter((f) => f.urgency === "fix");
  const openFix = toFix.filter((f) => f.state === "open");
  const pending = pendingComments(threads);
  const unresolved = (threads ?? []).filter((t) => !t.is_resolved && !t.comments.some((c) => c.pending)).length;
  const files = filesReviewed(manifest, tab.viewedFiles);
  const ci = ciStatus(checks);

  // Body, chosen verdict, and done state live on the tab (Tab.finishDraft /
  // finishDone), so switching tabs and back loses nothing.
  const draft = tab.finishDraft ?? {};
  const body = draft.body ?? "";
  const verb = draft.verb ?? defaultVerb(isMerged, openFix.length);
  const done = tab.finishDone ?? null;
  const [drafting, setDrafting] = useState(!draft.drafted);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [next, setNext] = useState<ReviewRequestItem | null | undefined>(undefined);
  const bodyRef = useRef(body);
  bodyRef.current = body;

  // Draft a body from fresh threads the first time the panel opens for this
  // review — never again on a remount, and never over text you've typed.
  useEffect(() => {
    if (draft.drafted) return;
    let live = true;
    props
      .onDraftBody(openFix)
      .then((text) => {
        if (live) props.onDraftChange({ body: mergeDraft(bodyRef.current, text), drafted: true });
      })
      .catch(() => {
        if (live) props.onDraftChange({ drafted: true });
      })
      .finally(() => {
        if (live) setDrafting(false);
      });
    return () => {
      live = false;
    };
  }, [tab.id]); // eslint-disable-line react-hooks/exhaustive-deps

  // After a submit the form unmounts; keep focus in the panel so Esc closes it.
  useEffect(() => {
    if (done) panelRef.current?.focus();
    if (done && next === undefined) props.onNextInQueue().then(setNext, () => setNext(null));
  }, [done != null]); // eslint-disable-line react-hooks/exhaustive-deps

  const blocker = submitBlocker(verb, body, pending.length, isMerged);

  async function submit() {
    if (blocker || submitting) return;
    setSubmitting(true);
    setError(null);
    // Counted now: submitting publishes them, and the refetch that follows
    // would read zero.
    const outcome = await attemptSubmit(props.onSubmit, verb, body, pending.length);
    setSubmitting(false);
    if (outcome.ok) props.onDone(outcome.done);
    else setError(outcome.error);
  }

  function onKey(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !done) {
      e.preventDefault();
      submit();
    }
  }

  const ALREADY: Record<string, string> = { approved: "approved", changes_requested: "requested changes on", commented: "commented on" };
  const already =
    tab.myReviewState && !tab.myReviewState.is_re_requested ? ALREADY[tab.myReviewState.status] ?? null : null;

  return (
    <Dialog
      label={`Finish review of #${manifest.pr_number}`}
      onClose={props.onClose}
      className="finish-panel"
      backdropClassName="finish-backdrop"
      panelRef={panelRef}
      onKeyDown={onKey}
    >
        <div className="finish-head">
          <div>
            <div className="finish-eyebrow">Finish review · #{manifest.pr_number}</div>
            <h2 className="finish-title">{done ? DONE_LABEL[done.event] : openFix.length > 0 ? `${openFix.length} still to fix` : "Ready to wrap up"}</h2>
          </div>
          <button className="inbox-btn inbox-btn--ghost" onClick={props.onClose} aria-label="Close">
            Close <kbd>esc</kbd>
          </button>
        </div>

        {done ? (
          <div className="finish-done">
            <p>
              {DONE_LABEL[done.event]} on {manifest.pr_title}.
              {done.posted > 0 && ` ${done.posted} batched comment${done.posted === 1 ? "" : "s"} went out with it.`}
            </p>
            {next === undefined ? (
              <p className="finish-muted">Looking for your next review…</p>
            ) : next ? (
              <button className="inbox-btn inbox-btn--primary finish-next" onClick={() => props.onOpenPr(`${next.owner}/${next.repo}#${next.number}`)}>
                Review next: {next.repo}#{next.number} · {next.title}
              </button>
            ) : (
              <p className="finish-muted">Your review queue is clear.</p>
            )}
            <div className="finish-actions">
              <button className="inbox-btn" onClick={props.onBackToQueue}>Back to queue</button>
              <button className="inbox-btn inbox-btn--ghost" onClick={props.onClose}>Stay on this PR</button>
            </div>
          </div>
        ) : (
          <>
            <section className="finish-section" aria-label="Recap">
              <div className="finish-lines">
                <div className={`finish-line finish-line--${openFix.length > 0 ? "fail" : "ok"}`}>{recapSummary(findings)}</div>
                <div className={`finish-line finish-line--${ci.tone}`}>{ci.text}</div>
                <div className="finish-line">
                  {files.reviewed} of {files.total} relevant files reviewed
                </div>
                {unresolved > 0 && <div className="finish-line finish-line--running">{unresolved} unresolved thread{unresolved === 1 ? "" : "s"}</div>}
                {already && <div className="finish-line finish-muted">You already {already} this PR. Submitting adds another review.</div>}
              </div>

              {findings.length > 0 && (
                <ul className="finish-findings">
                  {findings.map((f) => (
                    <li key={f.key}>
                      <button className={`finish-finding finish-finding--${f.state}`} onClick={() => props.onJumpToFinding(f)}>
                        <span className={`finish-mark finish-mark--${f.state}`}>{STATE_MARK[f.state]}</span>
                        <span className="finish-finding-title">{f.title}</span>
                        <span className={`finish-urgency finish-urgency--${f.urgency}`}>{f.urgency === "fix" ? "Fix" : "Look"}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section className="finish-section" aria-label="Your batched comments">
              <h3 className="finish-h">
                Your comments in this review <span className="finish-count">{pending.length}</span>
              </h3>
              {pending.length === 0 ? (
                <p className="finish-muted">None yet. Comments you add on the diff wait here and post when you submit.</p>
              ) : (
                <ul className="finish-comments">
                  {pending.map(({ thread, body: text }, i) => (
                    <li key={`${thread.id}-${i}`}>
                      <button className="finish-comment" onClick={() => props.onJumpToThread(thread)}>
                        <span className="inbox-loc">{fileName(thread.path)}{thread.line != null ? `:${thread.line}` : ""}</span>
                        <span className="finish-comment-body">{text.split("\n")[0]}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section className="finish-section" aria-label="Verdict">
              <div className="finish-verbs" role="radiogroup" aria-label="Review type">
                {VERBS.map((v) => {
                  const disabled = isMerged && v.event !== "COMMENT";
                  return (
                    <button
                      key={v.event}
                      role="radio"
                      aria-checked={verb === v.event}
                      disabled={disabled}
                      className={`finish-verb${verb === v.event ? " selected" : ""}`}
                      onClick={() => props.onDraftChange({ verb: v.event })}
                      title={disabled ? "This PR is merged" : undefined}
                    >
                      <b>{v.label}</b>
                      <span>{disabled ? "Unavailable: merged" : v.hint}</span>
                    </button>
                  );
                })}
              </div>
              <textarea
                className="finish-body"
                aria-label="Review body"
                value={body}
                placeholder={drafting ? "Drafting a summary from the open threads…" : "Leave a comment with your review"}
                onChange={(e) => props.onDraftChange({ body: e.target.value })}
                rows={5}
              />
              {error && <div className="finish-error" role="alert">Couldn't submit: {error}</div>}
              <div className="finish-actions">
                <button className="inbox-btn inbox-btn--primary" onClick={submit} disabled={!!blocker || submitting} title={blocker ?? undefined}>
                  {submitting ? "Submitting…" : `Submit ${VERBS.find((v) => v.event === verb)!.label.toLowerCase()}`} <kbd>⌘↵</kbd>
                </button>
                {blocker && <span className="finish-muted">{blocker}</span>}
              </div>
            </section>
          </>
        )}
    </Dialog>
  );
}

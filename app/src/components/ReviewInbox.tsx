// Inbox review layout (issue #238 phase 4, behind Settings → "Inbox review
// layout"). One review list — verdict, ranked findings, then the remaining
// files — beside a detail pane: the selected finding's card pinned above its
// diff. The list is the progress and the next step; j/k move through it and
// e / c / x act on the selected finding while the list has focus.

import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { FileDiff, NoteResolution, NoteResolutionState, PrChecksStatus, ReviewManifest, Tab } from "../types";
import { buildFindings, type Finding, type FindingKind } from "../review/findings";

const KIND_LABEL: Record<FindingKind, string> = {
  ci: "CI",
  bug: "Bug",
  behavior: "Behavior",
  risk: "Check",
  test_gap: "Test gap",
  simplification: "Simplify",
  observation: "Note",
  note: "Note",
  spec: "Spec",
};

const VERDICT_LABEL: Record<string, string> = {
  fix_first: "Fix first",
  ship: "Ship",
  needs_discussion: "Needs discussion",
};

const REASON_OPTIONS: { state: NoteResolutionState; label: string }[] = [
  { state: "noise", label: "Not a real issue" },
  { state: "intentional", label: "Intentional" },
  { state: "fixed", label: "Already fixed" },
];

export const INBOX_ABOUT = "about";

type Item =
  | { id: string; kind: "about" }
  | { id: string; kind: "finding"; finding: Finding }
  | { id: string; kind: "file"; file: FileDiff; group: string | null };

export interface ReviewInboxProps {
  tab: Tab & { manifest: ReviewManifest };
  checks: PrChecksStatus | null;
  viewerLogin: string | null;
  onSelectFinding: (f: Finding) => void;
  onSelectFile: (file: FileDiff) => void;
  onSelectPanel: (key: string) => void;
  onToggleViewed: (path: string) => void;
  onLooksFine: (f: Finding) => void;
  onNotAnIssue: (f: Finding, resolution: NoteResolution | null) => void;
  onReopen: (f: Finding) => void;
  onComment: (f: Finding) => void;
  /** Fetch review threads if not loaded yet (they drive the ✎ state). */
  onEnsureThreads: () => void;
  renderDiff: () => ReactNode;
  renderAbout: () => ReactNode;
  renderSpec: () => ReactNode;
  renderChecks: () => ReactNode;
}

function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

function location(f: Finding): string | null {
  if (!f.path) return null;
  return f.startLine != null ? `${fileName(f.path)}:${f.startLine}` : fileName(f.path);
}

function StateMark({ state }: { state: Finding["state"] }) {
  const glyph = state === "checked" ? "✓" : state === "commented" ? "✎" : state === "dismissed" ? "–" : "";
  const label = state === "open" ? "To do" : state === "checked" ? "Looks fine" : state === "commented" ? "Commented" : "Dismissed";
  return (
    <span className={`inbox-mark inbox-mark--${state}`} aria-label={label} title={label}>
      {glyph}
    </span>
  );
}

export function ReviewInbox(props: ReviewInboxProps) {
  const { tab, checks, viewerLogin, onEnsureThreads } = props;
  const manifest = tab.manifest;
  const [showNotRelevant, setShowNotRelevant] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (tab.commentThreads.status === "idle") onEnsureThreads();
  }, [tab.id, tab.commentThreads.status]); // eslint-disable-line react-hooks/exhaustive-deps

  const threads = tab.commentThreads.status === "loaded" ? tab.commentThreads.threads : undefined;
  const { findings, infoCountByPath } = useMemo(
    () =>
      buildFindings(manifest, {
        dismissed: tab.dismissedHighlights,
        checked: tab.checkedFindings,
        checks,
        resolvedSpecKeys: tab.resolvedSpecKeys,
        threads,
        viewerLogin,
      }),
    [manifest, tab.dismissedHighlights, tab.checkedFindings, checks, tab.resolvedSpecKeys, threads, viewerLogin],
  );

  // Files not already reachable through a finding, grouped by change group in
  // triage order; not-relevant files sit collapsed at the bottom.
  const { otherFiles, notRelevant } = useMemo(() => {
    const withFindings = new Set(findings.map((f) => f.path).filter(Boolean));
    const order = new Map((manifest.triage?.review_order ?? []).map((r, i) => [r.path, i]));
    const groupOf = new Map<string, string>();
    for (const g of manifest.change_groups ?? []) for (const p of g.file_paths) if (!groupOf.has(p)) groupOf.set(p, g.label);
    const groupRank = new Map((manifest.change_groups ?? []).map((g, i) => [g.label, i]));
    const rest = manifest.files
      .map((file, i) => ({ file, i }))
      .filter(({ file }) => file.classification !== "NOT_RELEVANT" && !withFindings.has(file.path))
      .sort((a, b) => {
        const ga = groupRank.get(groupOf.get(a.file.path) ?? "") ?? Infinity;
        const gb = groupRank.get(groupOf.get(b.file.path) ?? "") ?? Infinity;
        const oa = order.get(a.file.path) ?? Infinity;
        const ob = order.get(b.file.path) ?? Infinity;
        return (ga === gb ? 0 : ga - gb) || (oa === ob ? 0 : oa - ob) || a.i - b.i;
      })
      .map(({ file }) => ({ file, group: groupOf.get(file.path) ?? null }));
    return { otherFiles: rest, notRelevant: manifest.files.filter((f) => f.classification === "NOT_RELEVANT") };
  }, [manifest, findings]);

  const allItems: Item[] = useMemo(
    () => [
      { id: INBOX_ABOUT, kind: "about" as const },
      ...findings.map((f) => ({ id: f.key, kind: "finding" as const, finding: f })),
      ...otherFiles.map(({ file, group }) => ({ id: `file:${file.path}`, kind: "file" as const, file, group })),
      ...notRelevant.map((file) => ({ id: `file:${file.path}`, kind: "file" as const, file, group: null })),
    ],
    [findings, otherFiles, notRelevant],
  );
  // j/k walk only what's on screen.
  const navItems = showNotRelevant ? allItems : allItems.slice(0, allItems.length - notRelevant.length);

  const selection = tab.inboxSelection ?? null;
  const selected = allItems.find((i) => i.id === selection) ?? null;

  function select(item: Item) {
    if (item.kind === "finding") props.onSelectFinding(item.finding);
    else if (item.kind === "file") props.onSelectFile(item.file);
    else props.onSelectPanel(item.id);
  }

  // Land on the first open finding (else the first finding, else the first
  // file, else About) whenever nothing valid is selected.
  useEffect(() => {
    if (selected) return;
    const target =
      allItems.find((i) => i.kind === "finding" && i.finding.state === "open") ??
      allItems.find((i) => i.kind === "finding") ??
      allItems.find((i) => i.kind === "file" && i.file.classification !== "NOT_RELEVANT") ??
      allItems[0];
    if (target) select(target);
  }, [tab.id, selected == null]); // eslint-disable-line react-hooks/exhaustive-deps

  // Keep the selected row in view as j/k move.
  useEffect(() => {
    listRef.current?.querySelector(".inbox-row.selected")?.scrollIntoView({ block: "nearest" });
  }, [selection]);

  /** After acting on `from`, go to the next open finding (wrapping), else the
   * next item. The acted-on finding is excluded — its state hasn't committed. */
  function advance(from: Finding) {
    const idx = findings.findIndex((f) => f.key === from.key);
    for (let step = 1; step < findings.length; step++) {
      const f = findings[(idx + step) % findings.length];
      if (f.state === "open") return select({ id: f.key, kind: "finding", finding: f });
    }
    const pos = navItems.findIndex((i) => i.id === from.key);
    const next = navItems[pos + 1];
    if (next) select(next);
  }

  function act(kind: "fine" | "dismiss" | "comment", f: Finding, resolution: NoteResolution | null = null) {
    if (kind === "comment") return props.onComment(f);
    if (kind === "fine") props.onLooksFine(f);
    else props.onNotAnIssue(f, resolution);
    advance(f);
  }

  function onListKey(e: KeyboardEvent<HTMLDivElement>) {
    const t = e.target as HTMLElement;
    if (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || e.metaKey || e.ctrlKey || e.altKey) return;
    const pos = navItems.findIndex((i) => i.id === selection);
    const f = selected?.kind === "finding" ? selected.finding : null;
    let handled = true;
    switch (e.key) {
      case "j":
      case "ArrowDown":
        if (navItems[pos + 1]) select(navItems[pos + 1]);
        break;
      case "k":
      case "ArrowUp":
        if (pos > 0) select(navItems[pos - 1]);
        break;
      case "e":
        if (f && f.state !== "checked" && f.state !== "dismissed") act("fine", f);
        break;
      case "x":
        if (f && f.state !== "dismissed") act("dismiss", f);
        break;
      case "c":
        if (f) act("comment", f);
        break;
      default:
        handled = false;
    }
    // Stop here so the diff's own single-letter shortcuts (document listener)
    // don't also fire while the list has focus.
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  }

  const handled = findings.filter((f) => f.state !== "open").length;
  const verdict = manifest.review_verdict;
  let lastGroup: string | null | undefined;

  return (
    <div className="inbox">
      <div
        className="inbox-list"
        ref={listRef}
        tabIndex={0}
        role="listbox"
        aria-label="Review list"
        aria-activedescendant={selection ? `inbox-item-${selection}` : undefined}
        onKeyDown={onListKey}
      >
        {verdict && (
          <div className="inbox-verdict">
            <span className={`overview-verdict-chip overview-verdict-chip--${verdict.verdict}`}>
              {VERDICT_LABEL[verdict.verdict] ?? verdict.verdict}
            </span>
            {verdict.reason && <p>{verdict.reason}</p>}
          </div>
        )}

        <button
          id={`inbox-item-${INBOX_ABOUT}`}
          role="option"
          aria-selected={selection === INBOX_ABOUT}
          className={`inbox-row inbox-row--about${selection === INBOX_ABOUT ? " selected" : ""}`}
          onClick={() => props.onSelectPanel(INBOX_ABOUT)}
        >
          <span className="inbox-row-title">About this PR</span>
          <span className="inbox-row-meta">Summary, description, commits</span>
        </button>

        <div className="inbox-section">
          <span>Findings</span>
          <span className="inbox-progress" aria-label={`${handled} of ${findings.length} handled`}>
            <span className="inbox-meter">
              {findings.map((f) => (
                <i key={f.key} className={`inbox-meter-seg inbox-meter-seg--${f.state}`} />
              ))}
            </span>
            {handled}/{findings.length}
          </span>
        </div>
        {findings.length === 0 && <div className="inbox-empty">No findings — the AI flagged nothing to act on.</div>}
        {findings.map((f) => (
          <button
            key={f.key}
            id={`inbox-item-${f.key}`}
            role="option"
            aria-selected={selection === f.key}
            className={`inbox-row inbox-row--finding inbox-row--${f.state}${selection === f.key ? " selected" : ""}`}
            onClick={() => props.onSelectFinding(f)}
          >
            <StateMark state={f.state} />
            <span className="inbox-row-main">
              <span className="inbox-row-title">{f.title}</span>
              <span className="inbox-row-meta">
                <span className={`inbox-kind inbox-kind--${f.rank}`}>{KIND_LABEL[f.kind]}</span>
                {location(f) && <span className="inbox-loc">{location(f)}</span>}
              </span>
            </span>
          </button>
        ))}

        {otherFiles.length > 0 && (
          <div className="inbox-section">
            <span>Other relevant files</span>
            <span className="inbox-count">{otherFiles.length}</span>
          </div>
        )}
        {otherFiles.map(({ file, group }) => {
          const id = `file:${file.path}`;
          const header = group !== lastGroup && group ? <div className="inbox-group">{group}</div> : null;
          lastGroup = group;
          const viewed = tab.viewedFiles.has(file.path);
          const notes = infoCountByPath.get(file.path) ?? 0;
          return (
            <div key={id}>
              {header}
              <FileRow
                id={id}
                file={file}
                viewed={viewed}
                notes={notes}
                selected={selection === id}
                onSelect={() => props.onSelectFile(file)}
                onToggleViewed={() => props.onToggleViewed(file.path)}
              />
            </div>
          );
        })}

        {notRelevant.length > 0 && (
          <button className="inbox-section inbox-section--toggle" onClick={() => setShowNotRelevant((v) => !v)} aria-expanded={showNotRelevant}>
            <span>{showNotRelevant ? "▾" : "▸"} Not relevant</span>
            <span className="inbox-count">{notRelevant.length}</span>
          </button>
        )}
        {showNotRelevant &&
          notRelevant.map((file) => {
            const id = `file:${file.path}`;
            return (
              <FileRow
                key={id}
                id={id}
                file={file}
                viewed={tab.viewedFiles.has(file.path)}
                notes={0}
                selected={selection === id}
                onSelect={() => props.onSelectFile(file)}
                onToggleViewed={() => props.onToggleViewed(file.path)}
              />
            );
          })}

        <div className="inbox-keys" aria-hidden="true">
          <span><kbd>j</kbd><kbd>k</kbd> move</span>
          <span><kbd>e</kbd> looks fine</span>
          <span><kbd>c</kbd> comment</span>
          <span><kbd>x</kbd> not an issue</span>
        </div>
      </div>

      <div className="inbox-detail">
        {selected?.kind === "about" && <div className="inbox-panel">{props.renderAbout()}</div>}
        {selected?.kind === "finding" && (
          <>
            <FindingCard
              finding={selected.finding}
              onLooksFine={() => act("fine", selected.finding)}
              onComment={() => act("comment", selected.finding)}
              onNotAnIssue={(r) => act("dismiss", selected.finding, r)}
              onReopen={() => props.onReopen(selected.finding)}
            />
            {selected.finding.kind === "spec" ? (
              <div className="inbox-panel">{props.renderSpec()}</div>
            ) : selected.finding.kind === "ci" ? (
              <div className="inbox-panel inbox-panel--flush">{props.renderChecks()}</div>
            ) : (
              <div className="inbox-diff">{props.renderDiff()}</div>
            )}
          </>
        )}
        {selected?.kind === "file" && <div className="inbox-diff">{props.renderDiff()}</div>}
        {!selected && <div className="no-file-selected">Select an item to review</div>}
      </div>
    </div>
  );
}

function FileRow({ id, file, viewed, notes, selected, onSelect, onToggleViewed }: {
  id: string;
  file: FileDiff;
  viewed: boolean;
  notes: number;
  selected: boolean;
  onSelect: () => void;
  onToggleViewed: () => void;
}) {
  return (
    <div className={`inbox-row inbox-row--file${selected ? " selected" : ""}${viewed ? " viewed" : ""}`}>
      <input
        type="checkbox"
        className="inbox-viewed"
        checked={viewed}
        onChange={onToggleViewed}
        aria-label={`Mark ${fileName(file.path)} ${viewed ? "unreviewed" : "reviewed"}`}
      />
      <button id={`inbox-item-${id}`} role="option" aria-selected={selected} className="inbox-file-btn" onClick={onSelect} title={file.path}>
        <span className="inbox-file-name">{fileName(file.path)}</span>
        <span className="inbox-file-stat">
          <span className="inbox-add">+{file.additions}</span> <span className="inbox-del">−{file.deletions}</span>
          {notes > 0 && <span className="inbox-notes">{notes} note{notes === 1 ? "" : "s"}</span>}
        </span>
      </button>
    </div>
  );
}

function FindingCard({ finding: f, onLooksFine, onComment, onNotAnIssue, onReopen }: {
  finding: Finding;
  onLooksFine: () => void;
  onComment: () => void;
  onNotAnIssue: (resolution: NoteResolution | null) => void;
  onReopen: () => void;
}) {
  const [choosing, setChoosing] = useState(false);
  const [reason, setReason] = useState("");
  useEffect(() => {
    setChoosing(false);
    setReason("");
  }, [f.key]);

  const loc = f.path ? `${f.path}${f.startLine != null ? `:${f.startLine}` : ""}` : null;
  const showDetail = f.detail && f.detail.trim() !== f.title.trim();

  return (
    <section className={`inbox-card inbox-card--${f.rank}`} aria-label="Selected finding">
      <div className="inbox-card-head">
        <span className={`inbox-kind inbox-kind--${f.rank}`}>{KIND_LABEL[f.kind]}</span>
        {loc && <span className="inbox-loc" title={loc}>{loc}</span>}
      </div>
      <h3 className="inbox-card-title">{f.title}</h3>
      {showDetail && <p className="inbox-card-text">{f.detail}</p>}
      {f.riskDetail && <p className="inbox-card-text inbox-card-why"><span>Why it was flagged</span>{f.riskDetail}</p>}
      {(f.scenario || f.fix) && (
        <dl className="inbox-card-dl">
          {f.scenario && (<><dt>Scenario</dt><dd>{f.scenario}</dd></>)}
          {f.fix && (<><dt>Fix</dt><dd>{f.fix}</dd></>)}
        </dl>
      )}
      {f.kind === "ci" && f.items && <p className="inbox-card-text">Failing: {f.items.join(", ")}</p>}

      {f.state === "checked" || f.state === "dismissed" ? (
        <div className="inbox-card-actions">
          <span className="inbox-card-state">{f.state === "checked" ? "✓ Marked looks fine" : "– Marked not an issue"}</span>
          <button className="inbox-btn inbox-btn--ghost" onClick={onReopen}>Reopen</button>
        </div>
      ) : choosing ? (
        <div className="inbox-card-actions inbox-card-choose">
          {REASON_OPTIONS.map((o) => (
            <button key={o.state} className="inbox-btn" onClick={() => onNotAnIssue({ state: o.state, reason: reason.trim() })}>
              {o.label}
            </button>
          ))}
          <input
            className="inbox-reason"
            placeholder="Why? (optional — feeds future AI runs)"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
          <button className="inbox-btn inbox-btn--ghost" onClick={() => setChoosing(false)}>Cancel</button>
        </div>
      ) : (
        <div className="inbox-card-actions">
          <button className="inbox-btn inbox-btn--primary" onClick={onLooksFine}>Looks fine <kbd>e</kbd></button>
          <button className="inbox-btn" onClick={onComment}>Comment <kbd>c</kbd></button>
          <button className="inbox-btn" onClick={() => (f.kind === "spec" ? onNotAnIssue(null) : setChoosing(true))}>
            Not an issue <kbd>x</kbd>
          </button>
          {f.state === "commented" && <span className="inbox-card-state">✎ You commented here</span>}
        </div>
      )}
    </section>
  );
}

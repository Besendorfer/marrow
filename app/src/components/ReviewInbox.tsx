// Inbox review layout (issue #238): the default; Settings → "Use the classic
// layout" goes back to the Overview/Files/Commits/Checks lenses. One review
// list — verdict, About/Commits/Checks, ranked findings, then the remaining
// files — beside a detail pane: the selected finding's card pinned above its
// diff. The list is the progress and the next step; j/k move through it and
// e / c / x act on the selected finding while the list has focus. Only the
// selected row is in the Tab order (roving focus): Tab leaves the list, Enter
// hands the keyboard to the diff, Esc there comes back.

import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { FileDiff, NoteResolution, NoteResolutionState, PrChecksStatus, PrCommit, ReviewManifest, Tab } from "../types";
import { buildFindings, findingClaim, selectionIdFor, type Finding, type FindingKind } from "../review/findings";
import { chooserKeyAction, landingId, listKeyAction, nextAfterAction } from "../review/inboxKeys";
import { ciStatus } from "../review/finish";
import { INBOX_ABOUT, INBOX_CHECKS, INBOX_COMMITS } from "../review/navigation";

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

/** A review-list entry. `hidden` file items back selections of files that
 * already appear through their findings (opened via search, chat, About…),
 * so those selections resolve instead of reading as "nothing selected". */
type Item =
  | { id: string; kind: "panel" }
  | { id: string; kind: "finding"; finding: Finding }
  | { id: string; kind: "file"; file: FileDiff; group: string | null; hidden?: boolean };

export interface ReviewInboxProps {
  tab: Tab & { manifest: ReviewManifest };
  checks: PrChecksStatus | null;
  viewerLogin: string | null;
  onSelectFinding: (f: Finding) => void;
  onSelectFile: (file: FileDiff) => void;
  onSelectPanel: (key: string) => void;
  onToggleViewed: (path: string) => void;
  /** Returns whether a mark was made — the list advances only then. */
  onLooksFine: (f: Finding) => boolean;
  onNotAnIssue: (f: Finding, resolution: NoteResolution | null) => void;
  onReopen: (f: Finding) => void;
  onComment: (f: Finding) => void;
  /** Fetch review threads if not loaded yet (they drive the ✎ state). */
  onEnsureThreads: () => void;
  renderDiff: () => ReactNode;
  renderAbout: () => ReactNode;
  renderSpec: () => ReactNode;
  renderChecks: () => ReactNode;
  renderCommits: () => ReactNode;
  onSelectCommit: (commit: PrCommit) => void;
  onFinish: () => void;
  /** The divider between the list and the detail pane (resizes the list). */
  listSplitter?: ReactNode;
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
  // The "Not an issue" reason picker, lifted here so the button and the x key
  // behave the same: both open it, then 1–3 / x / Enter / Esc drive it.
  const [choosingKey, setChoosingKey] = useState<string | null>(null);
  const [reason, setReason] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);
  // Set when focus should follow the selection even though it's outside the
  // list right now (an action taken from the card's buttons).
  const refocusRef = useRef(false);

  useEffect(() => {
    if (tab.commentThreads.status === "idle") onEnsureThreads();
  }, [tab.id, tab.commentThreads.status]); // eslint-disable-line react-hooks/exhaustive-deps

  const threads = tab.commentThreads.status === "loaded" ? tab.commentThreads.threads : undefined;
  const { findings: rankedFindings, infoCountByPath } = useMemo(
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
  // "Fix before merge" leads, then "Worth a look" — list, j/k, and advance
  // all follow this order. A finding grouped under another (Jev, issue #249)
  // sits in its primary's section, right after it, whatever its own urgency.
  const { findings, sectionOf } = useMemo(() => {
    const byKey = new Map(rankedFindings.map((f) => [f.key, f]));
    const sectionOf = (f: Finding) => (f.parentKey ? byKey.get(f.parentKey)?.urgency : undefined) ?? f.urgency;
    return {
      findings: [...rankedFindings.filter((f) => sectionOf(f) === "fix"), ...rankedFindings.filter((f) => sectionOf(f) === "look")],
      sectionOf,
    };
  }, [rankedFindings]);
  const toFix = findings.filter((f) => sectionOf(f) === "fix");
  const toLook = findings.filter((f) => sectionOf(f) === "look");

  // Files not already reachable through a finding, grouped by change group in
  // triage order; not-relevant files sit collapsed at the bottom.
  const { otherFiles, notRelevant, filesWithFindings } = useMemo(() => {
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
    return {
      otherFiles: rest,
      notRelevant: manifest.files.filter((f) => f.classification === "NOT_RELEVANT"),
      filesWithFindings: manifest.files.filter((f) => withFindings.has(f.path) && f.classification !== "NOT_RELEVANT"),
    };
  }, [manifest, findings]);

  const allItems: Item[] = useMemo(
    () => [
      { id: INBOX_ABOUT, kind: "panel" as const },
      { id: INBOX_COMMITS, kind: "panel" as const },
      { id: INBOX_CHECKS, kind: "panel" as const },
      ...findings.map((f) => ({ id: selectionIdFor(f), kind: "finding" as const, finding: f })),
      ...otherFiles.map(({ file, group }) => ({ id: `file:${file.path}`, kind: "file" as const, file, group })),
      ...notRelevant.map((file) => ({ id: `file:${file.path}`, kind: "file" as const, file, group: null })),
      ...filesWithFindings.map((file) => ({ id: `file:${file.path}`, kind: "file" as const, file, group: null, hidden: true })),
    ],
    [findings, otherFiles, notRelevant, filesWithFindings],
  );
  // j/k walk only what's on screen.
  const navItems = allItems.filter(
    (i) => !(i.kind === "file" && (i.hidden || (!showNotRelevant && i.file.classification === "NOT_RELEVANT"))),
  );

  const selection = tab.inboxSelection ?? null;
  const selected = allItems.find((i) => i.id === selection) ?? null;

  function select(item: Item) {
    if (item.kind === "finding") props.onSelectFinding(item.finding);
    else if (item.kind === "file") props.onSelectFile(item.file);
    else props.onSelectPanel(item.id);
  }

  // Land somewhere whenever nothing valid is selected (see landingId).
  useEffect(() => {
    if (selected) return;
    const id = landingId(
      allItems.map((i) => ({
        id: i.id,
        kind: i.kind,
        state: i.kind === "finding" ? i.finding.state : undefined,
        notRelevant: i.kind === "file" && i.file.classification === "NOT_RELEVANT",
      })),
      tab.lens,
      { commits: INBOX_COMMITS, checks: INBOX_CHECKS },
    );
    const target = allItems.find((i) => i.id === id);
    if (target) select(target);
    // allItems too: a selection that didn't resolve when it was made (items
    // still settling after a restore or refresh) must get another chance, or
    // the pane stays on "Select an item" with nothing re-running. No loop —
    // allItems is memoized and only changes with the underlying data.
  }, [tab.id, selected == null, allItems]); // eslint-disable-line react-hooks/exhaustive-deps

  // A not-relevant file selected from elsewhere reveals its section.
  useEffect(() => {
    if (selected?.kind === "file" && selected.file.classification === "NOT_RELEVANT") setShowNotRelevant(true);
  }, [selected]);

  // Moving on closes an open reason picker.
  useEffect(() => {
    setChoosingKey(null);
    setReason("");
  }, [selection]);

  /** Focus the selected row — the list's one Tab stop. */
  function focusRow() {
    listRef.current?.querySelector<HTMLElement>('[data-row][tabindex="0"]')?.focus({ preventScroll: true });
  }

  // Keep the selected row in view as j/k move, and keep focus on it while the
  // keyboard is in the list.
  useEffect(() => {
    listRef.current?.querySelector(".inbox-row.selected")?.scrollIntoView({ block: "nearest" });
    if (refocusRef.current || listRef.current?.contains(document.activeElement)) focusRow();
    refocusRef.current = false;
  }, [selection]);

  /** List position of the selection; a hidden file item sits at its first finding. */
  function selectionPos(): number {
    if (selected?.kind === "file" && selected.hidden) {
      return navItems.findIndex((i) => i.kind === "finding" && i.finding.path === selected.file.path);
    }
    return navItems.findIndex((i) => i.id === selection);
  }

  /** After acting on `from`, go to the next open finding (wrapping), else the
   * next item. The acted-on finding is excluded — its state hasn't committed. */
  function advance(from: Finding) {
    const id = nextAfterAction(
      findings.map((f) => ({ id: selectionIdFor(f), state: f.state })),
      navItems.map((i) => i.id),
      selectionIdFor(from),
    );
    const next = id ? allItems.find((i) => i.id === id) : undefined;
    if (next) select(next);
  }

  function act(kind: "fine" | "dismiss" | "comment", f: Finding, resolution: NoteResolution | null = null) {
    if (kind === "comment") return props.onComment(f);
    if (kind === "fine") {
      if (!props.onLooksFine(f)) return;
    } else {
      props.onNotAnIssue(f, resolution);
    }
    // Card buttons live outside the list; hand focus back so j/k/e/c/x keep
    // working — now, and again once the advanced-to row renders.
    refocusRef.current = true;
    focusRow();
    advance(f);
  }

  function onListKey(e: KeyboardEvent<HTMLDivElement>) {
    const t = e.target as HTMLElement;
    const typing = t.tagName === "TEXTAREA" || (t.tagName === "INPUT" && (t as HTMLInputElement).type !== "checkbox");
    if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
    // Enter on a non-row control (a checkbox, the Not relevant toggle) keeps
    // its own meaning.
    if (e.key === "Enter" && !t.hasAttribute("data-row")) return;
    const f = selected?.kind === "finding" ? selected.finding : null;
    if (f && choosingKey === f.key) {
      const pick = chooserKeyAction(e.key, REASON_OPTIONS.length);
      if (pick) {
        e.preventDefault();
        e.stopPropagation();
        if (pick.type === "cancel") setChoosingKey(null);
        else dismissWith(f, REASON_OPTIONS[pick.index].state);
        return;
      }
    }
    const action = listKeyAction(e.key, f);
    if (!action) return;
    // Stop here so the diff's own single-letter shortcuts (a bubble-phase
    // document listener) don't also fire while the list has focus.
    e.preventDefault();
    e.stopPropagation();
    if (action.type === "move") {
      const pos = selectionPos();
      const next = navItems[pos + action.delta];
      if (next && pos + action.delta >= 0) select(next);
      else if (pos < 0 && navItems[0]) select(navItems[0]);
    } else if (action.type === "edge") {
      const target = action.to === "first" ? navItems[0] : navItems[navItems.length - 1];
      if (target) select(target);
    } else if (action.type === "diff") {
      detailRef.current?.focus();
    } else if (action.type === "finish") {
      props.onFinish();
    } else if (f) {
      if (action.type === "dismiss" && f.kind !== "spec") startChoosing(f);
      else act(action.type, f);
    }
  }

  function startChoosing(f: Finding) {
    setChoosingKey(f.key);
    focusRow();
  }

  /** Esc from the detail pane (not from a field in it) returns to the list. */
  function onDetailKey(e: KeyboardEvent<HTMLDivElement>) {
    const t = e.target as HTMLElement;
    if (e.key !== "Escape" || t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable) return;
    e.preventDefault();
    e.stopPropagation();
    focusRow();
  }

  function dismissWith(f: Finding, state: NoteResolutionState) {
    const why = reason.trim();
    setChoosingKey(null);
    setReason("");
    act("dismiss", f, { state, reason: why });
  }

  // Counts go by each finding's own urgency, not the section it's grouped into.
  const openFix = findings.filter((f) => f.urgency === "fix" && f.state === "open").length;
  const openLook = findings.filter((f) => f.urgency === "look" && f.state === "open").length;
  const summary =
    findings.length === 0
      ? "No findings"
      : [openFix === 0 ? "Nothing to fix" : `${openFix} to fix`, openLook > 0 ? `${openLook} worth a look` : null].filter(Boolean).join(" · ");
  const verdict = manifest.review_verdict;
  const ci = ciStatus(checks);
  let lastGroup: string | null | undefined;

  function renderFindingRow(f: Finding) {
    const id = selectionIdFor(f);
    // A file opened from elsewhere (a hidden item) highlights its first finding.
    const isSel =
      selection === id ||
      (selected?.kind === "file" && !!selected.hidden && selected.file.path === f.path && findings.find((x) => x.path === f.path) === f);
    return (
      <button
        key={f.key}
        data-row
        tabIndex={isSel ? 0 : -1}
        aria-current={isSel ? "true" : undefined}
        className={`inbox-row inbox-row--finding inbox-row--${f.state}${f.parentKey ? " inbox-row--child" : ""}${isSel ? " selected" : ""}`}
        onClick={() => props.onSelectFinding(f)}
      >
        <StateMark state={f.state} />
        <span className="inbox-row-main">
          <span className="inbox-row-title">{f.title}</span>
          <span className="inbox-row-meta">
            {f.parentKey && <span className="inbox-rel" title="Same root cause as the finding above, different action">Related</span>}
            {f.parentKey && f.urgency !== sectionOf(f) && (
              <span className="inbox-rel" title="Its own urgency, which the counts above use">{f.urgency === "fix" ? "Fix" : "Look"}</span>
            )}
            <span className={`inbox-kind inbox-kind--${f.rank}`}>{KIND_LABEL[f.kind]}</span>
            {location(f) && <span className="inbox-loc">{location(f)}</span>}
            {f.duplicates?.length ? (
              <span className="inbox-rel" title="The same problem was reported more than once; merged here">
                +{f.duplicates.length} duplicate{f.duplicates.length === 1 ? "" : "s"}
              </span>
            ) : null}
          </span>
        </span>
      </button>
    );
  }

  return (
    <div className="inbox">
      <div
        className="inbox-list"
        ref={listRef}
        role="navigation"
        aria-label="Review list"
        onKeyDown={onListKey}
      >
        <div className="inbox-verdict">
          <div className="inbox-verdict-line">
            {verdict && (
              <span className={`overview-verdict-chip overview-verdict-chip--${verdict.verdict}`}>
                {VERDICT_LABEL[verdict.verdict] ?? verdict.verdict}
              </span>
            )}
            <span className={`inbox-summary${openFix > 0 ? " inbox-summary--fix" : ""}`}>{summary}</span>
          </div>
          {verdict?.reason && <p>{verdict.reason}</p>}
        </div>

        <div className="inbox-panels">
        <PanelRow id={INBOX_ABOUT} selection={selection} onSelect={props.onSelectPanel} title="About this PR" meta="Summary and description" />
        <PanelRow
          id={INBOX_COMMITS}
          selection={selection}
          onSelect={props.onSelectPanel}
          title="Commits"
          meta={`${manifest.commits.length} commit${manifest.commits.length === 1 ? "" : "s"}`}
        />
        {selection === INBOX_COMMITS && manifest.commits.length > 0 && (
          // Newest first, like GitHub's list; the pane shows the picked one.
          <div className="inbox-commits" role="group" aria-label="Commits in this PR">
            {[...manifest.commits].reverse().map((c) => {
              const isSel = c.sha === tab.selectedCommit?.sha;
              return (
                <button
                  key={c.sha}
                  tabIndex={-1}
                  aria-current={isSel ? "true" : undefined}
                  className={`inbox-commit${isSel ? " selected" : ""}`}
                  onClick={() => props.onSelectCommit(c)}
                  title={c.message_headline}
                >
                  <span className="inbox-commit-msg">{c.message_headline}</span>
                  <span className="inbox-loc">{c.sha.slice(0, 7)}</span>
                </button>
              );
            })}
          </div>
        )}
        <PanelRow id={INBOX_CHECKS} selection={selection} onSelect={props.onSelectPanel} title="Checks" meta={ci.text} tone={ci.tone} />
        </div>

        <FindingSection
          title="Fix before merge"
          items={toFix}
          empty={findings.length === 0 ? "No findings — the AI flagged nothing to act on." : "Nothing to fix — the AI claims no defects."}
          renderRow={renderFindingRow}
        />
        {toLook.length > 0 && <FindingSection title="Worth a look" items={toLook} renderRow={renderFindingRow} />}

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
          <span><kbd>↵</kbd> to diff · <kbd>esc</kbd> back</span>
          <span><kbd>f</kbd> finish</span>
        </div>
      </div>

      {props.listSplitter}
      <div className="inbox-detail" ref={detailRef} tabIndex={-1} role="region" aria-label="Review detail" onKeyDown={onDetailKey}>
        {selected?.id === INBOX_ABOUT && <div className="inbox-panel">{props.renderAbout()}</div>}
        {selected?.id === INBOX_COMMITS && <div className="inbox-panel inbox-panel--commits">{props.renderCommits()}</div>}
        {selected?.id === INBOX_CHECKS && <div className="inbox-panel inbox-panel--flush">{props.renderChecks()}</div>}
        {selected?.kind === "finding" && (
          <>
            <FindingCard
              finding={selected.finding}
              onLooksFine={() => act("fine", selected.finding)}
              onComment={() => act("comment", selected.finding)}
              choosing={choosingKey === selected.finding.key}
              reason={reason}
              onReason={setReason}
              onStartChoosing={() => (selected.finding.kind === "spec" ? act("dismiss", selected.finding) : startChoosing(selected.finding))}
              onCancelChoosing={() => setChoosingKey(null)}
              onPickReason={(state) => dismissWith(selected.finding, state)}
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

function PanelRow({ id, selection, onSelect, title, meta, tone }: {
  id: string;
  selection: string | null;
  onSelect: (id: string) => void;
  title: string;
  meta: string;
  tone?: "ok" | "running" | "fail" | "none";
}) {
  const isSel = selection === id;
  return (
    <button
      data-row
      tabIndex={isSel ? 0 : -1}
      aria-current={isSel ? "true" : undefined}
      className={`inbox-row inbox-row--panel${isSel ? " selected" : ""}`}
      onClick={() => onSelect(id)}
    >
      <span className="inbox-row-title">{title}</span>
      <span className={`inbox-row-meta${tone ? ` inbox-tone--${tone}` : ""}`}>{meta}</span>
    </button>
  );
}

function FindingSection({ title, items, empty, renderRow }: {
  title: string;
  items: Finding[];
  empty?: string;
  renderRow: (f: Finding) => ReactNode;
}) {
  const handled = items.filter((f) => f.state !== "open").length;
  return (
    <>
      <div className="inbox-section">
        <span>{title}</span>
        {items.length > 0 && (
          <span className="inbox-progress" aria-label={`${handled} of ${items.length} handled`}>
            <span className="inbox-meter">
              {items.map((f) => (
                <i key={f.key} className={`inbox-meter-seg inbox-meter-seg--${f.state}`} />
              ))}
            </span>
            {handled}/{items.length}
          </span>
        )}
      </div>
      {items.length === 0 && empty && <div className="inbox-empty">{empty}</div>}
      {items.map(renderRow)}
    </>
  );
}

function FileRow({ file, viewed, notes, selected, onSelect, onToggleViewed }: {
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
        tabIndex={-1}
        checked={viewed}
        onChange={onToggleViewed}
        aria-label={`Mark ${fileName(file.path)} ${viewed ? "unreviewed" : "reviewed"}`}
      />
      <button data-row tabIndex={selected ? 0 : -1} aria-current={selected ? "true" : undefined} className="inbox-file-btn" onClick={onSelect} title={file.path}>
        <span className="inbox-file-name">{fileName(file.path)}</span>
        <span className="inbox-file-stat">
          <span className="inbox-add">+{file.additions}</span> <span className="inbox-del">−{file.deletions}</span>
          {notes > 0 && <span className="inbox-notes">{notes} note{notes === 1 ? "" : "s"}</span>}
        </span>
      </button>
    </div>
  );
}

function FindingCard({ finding: f, onLooksFine, onComment, onReopen, choosing, reason, onReason, onStartChoosing, onCancelChoosing, onPickReason }: {
  finding: Finding;
  onLooksFine: () => void;
  onComment: () => void;
  onReopen: () => void;
  choosing: boolean;
  reason: string;
  onReason: (text: string) => void;
  onStartChoosing: () => void;
  onCancelChoosing: () => void;
  onPickReason: (state: NoteResolutionState) => void;
}) {
  const loc = f.path ? `${f.path}${f.startLine != null ? `:${f.startLine}` : ""}` : null;
  const showDetail = f.detail && f.detail.trim() !== f.title.trim();

  return (
    <section className={`inbox-card inbox-card--${f.rank}`} aria-label="Selected finding">
      <div className="inbox-card-head">
        <span className={`inbox-kind inbox-kind--${f.rank}`}>{KIND_LABEL[f.kind]}</span>
        {loc && <span className="inbox-loc" title={loc}>{loc}</span>}
      </div>
      <h3 className="inbox-card-title">{f.title}</h3>
      <p className={`inbox-card-claim inbox-card-claim--${f.urgency}`}>{findingClaim(f)}</p>
      {showDetail && <p className="inbox-card-text">{f.detail}</p>}
      {f.riskDetail && <p className="inbox-card-text inbox-card-why"><span>Why it was flagged</span>{f.riskDetail}</p>}
      {(f.scenario || f.fix) && (
        <dl className="inbox-card-dl">
          {f.scenario && (<><dt>Scenario</dt><dd>{f.scenario}</dd></>)}
          {f.fix && (<><dt>Fix</dt><dd>{f.fix}</dd></>)}
        </dl>
      )}
      {f.kind === "ci" && f.items && <p className="inbox-card-text">Failing: {f.items.join(", ")}</p>}
      {f.duplicates?.length ? (
        <div className="inbox-card-also">
          <span>Also reported as</span>
          <ul>
            {f.duplicates.map((d) => (
              <li key={d.key}>
                {d.detail ?? d.title}
                {d.path && <span className="inbox-loc"> {fileName(d.path)}{d.startLine != null ? `:${d.startLine}` : ""}</span>}
              </li>
            ))}
          </ul>
          <small>Merged by Jev as the same problem. Your verdict here applies to both.</small>
        </div>
      ) : null}

      {f.state === "checked" || f.state === "dismissed" ? (
        <div className="inbox-card-actions">
          <span className="inbox-card-state">{f.state === "checked" ? "✓ Marked looks fine" : "– Marked not an issue"}</span>
          <button className="inbox-btn inbox-btn--ghost" onClick={onReopen}>Reopen</button>
        </div>
      ) : choosing ? (
        <div className="inbox-card-actions inbox-card-choose">
          {REASON_OPTIONS.map((o, i) => (
            <button key={o.state} className="inbox-btn" onClick={() => onPickReason(o.state)}>
              {o.label} <kbd>{i + 1}</kbd>
            </button>
          ))}
          <input
            className="inbox-reason"
            placeholder="Why? (optional — feeds future AI runs)"
            value={reason}
            onChange={(e) => onReason(e.target.value)}
          />
          <button className="inbox-btn inbox-btn--ghost" onClick={onCancelChoosing}>Cancel <kbd>esc</kbd></button>
        </div>
      ) : f.kind === "spec" ? (
        // A spec finding has one verdict: its requirements are addressed
        // (the per-requirement store) — e and x both do that.
        <div className="inbox-card-actions">
          <button className="inbox-btn inbox-btn--primary" onClick={onLooksFine}>Mark addressed <kbd>e</kbd></button>
          <button className="inbox-btn" onClick={onComment}>Comment <kbd>c</kbd></button>
        </div>
      ) : (
        <div className="inbox-card-actions">
          <button className="inbox-btn inbox-btn--primary" onClick={onLooksFine}>Looks fine <kbd>e</kbd></button>
          <button className="inbox-btn" onClick={onComment}>Comment <kbd>c</kbd></button>
          <button className="inbox-btn" onClick={onStartChoosing}>
            Not an issue <kbd>x</kbd>
          </button>
          {f.state === "commented" && <span className="inbox-card-state">✎ You commented here</span>}
        </div>
      )}
    </section>
  );
}

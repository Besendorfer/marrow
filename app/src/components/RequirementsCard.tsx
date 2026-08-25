import { useEffect, useRef, useState } from "react";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import type { NoteResolution, RequirementEntry, ReviewManifest, TestRef } from "../types";
import { specResolveKey } from "./digest";

interface RequirementsCardProps {
  manifest: ReviewManifest;
  resolvedSpecKeys?: Set<string>;
  specResolutions?: Map<string, NoteResolution>;
  onResolveSpec?: (key: string) => void;
  onRestoreSpec?: (key: string) => void;
  onOpenAt?: (path: string, line?: number) => void;
  /** User-provided requirements text saved locally (issue #179 phase 2), or
   * `null` if nothing's been saved yet. Pre-fills the empty-state editor. */
  localRequirements: string | null;
  /** Persist `text` as this PR's local requirements. Saving triggers a
   * coverage-only re-analysis immediately (see App.saveLocalRequirements). */
  onSaveRequirements: (text: string) => void;
  /** True while the save-triggered coverage re-analysis is running. */
  analyzing?: boolean;
}

function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

const STATUS_GLYPH: Record<RequirementEntry["status"], string> = {
  covered: "✓",
  partial: "◐",
  uncovered: "○",
  untestable: "—",
};

/** A requirement row, or an orphan-test footer row — same expand-in-place
 * grammar as AttentionDigest's DigestRow (wrapper + toggle button, claim
 * clamped to 2 lines collapsed, full when expanded, detail = note). */
function RequirementRow({
  req,
  addressed,
  expanded,
  onToggle,
  onOpenAt,
  onResolveSpec,
  onRestoreSpec,
}: {
  req: RequirementEntry;
  addressed: boolean;
  expanded: boolean;
  onToggle: () => void;
  onOpenAt?: (path: string, line?: number) => void;
  onResolveSpec?: (key: string) => void;
  onRestoreSpec?: (key: string) => void;
}) {
  const key = specResolveKey(req.text);
  const firstTest = req.tests[0];
  const resolvable = req.status === "uncovered" || req.status === "partial";
  const canResolve = resolvable && !addressed && !!onResolveSpec;
  const canRestore = resolvable && addressed && !!onRestoreSpec;
  const rowClass = ["overview-digest-row", expanded && "expanded", addressed && "resolved"]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={rowClass}>
      <button className="overview-digest-row-toggle" aria-expanded={expanded} onClick={onToggle}>
        {/* An addressed row reads as checked off — the disposition trumps the
          * coverage status in the glyph; the affix still says why it's dim. */}
        <span className={addressed ? "req-glyph req-glyph--addressed" : `req-glyph req-glyph--${req.status}`}>
          {addressed ? "✓" : STATUS_GLYPH[req.status]}
        </span>
        <div className="overview-digest-row-main">
          <div className="overview-digest-row-top">
            <span className="overview-digest-row-claim">{req.text}</span>
            {addressed && <span className="req-card-row-affix">addressed</span>}
          </div>
          {req.note && <span className="overview-digest-row-detail">{req.note}</span>}
        </div>
      </button>
      {expanded && (firstTest || canResolve || canRestore) && (
        <div className="overview-digest-row-actions">
          {firstTest && (
            <button
              className="overview-digest-row-action"
              onClick={() => onOpenAt?.(firstTest.path)}
            >
              Open {fileName(firstTest.path)} →
            </button>
          )}
          {canResolve && (
            <button className="overview-digest-row-action" onClick={() => onResolveSpec!(key)}>
              Mark addressed ✓
            </button>
          )}
          {canRestore && (
            <button className="overview-digest-row-action" onClick={() => onRestoreSpec!(key)}>
              Restore
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function OrphanTestRow({
  test,
  expanded,
  onToggle,
  onOpenAt,
}: {
  test: TestRef;
  expanded: boolean;
  onToggle: () => void;
  onOpenAt?: (path: string, line?: number) => void;
}) {
  const rowClass = ["overview-digest-row", expanded && "expanded"].filter(Boolean).join(" ");
  return (
    <div className={rowClass}>
      <button className="overview-digest-row-toggle" aria-expanded={expanded} onClick={onToggle}>
        <span className="digest-dot digest-dot-info" />
        <div className="overview-digest-row-main">
          <div className="overview-digest-row-top">
            <span className="overview-digest-row-claim">
              Test without a stated requirement: {fileName(test.path)}
            </span>
          </div>
          {test.note && <span className="overview-digest-row-detail">{test.note}</span>}
        </div>
      </button>
      {expanded && (
        <div className="overview-digest-row-actions">
          <button className="overview-digest-row-action" onClick={() => onOpenAt?.(test.path)}>
            Open {fileName(test.path)} →
          </button>
        </div>
      )}
    </div>
  );
}

function RequirementsEditor({
  draft,
  setDraft,
  prUrl,
  onSave,
  onCancel,
  onClear,
  analyzing,
}: {
  draft: string;
  setDraft: (text: string) => void;
  prUrl: string;
  onSave: () => void;
  analyzing?: boolean;
  /** Present only in edit mode (the empty state has nothing to go back to). */
  onCancel?: () => void;
  /** Present only when local requirements exist — reverts to body extraction. */
  onClear?: () => void;
}) {
  return (
    <>
      <textarea
        className="requirements-card-textarea"
        value={draft}
        placeholder="Paste or write the requirements/acceptance criteria for this PR…"
        onChange={(e) => setDraft(e.target.value)}
      />
      <div className="requirements-card-editor-actions">
        <button
          className="requirements-card-save"
          onClick={onSave}
          disabled={analyzing || draft.trim().length === 0}
        >
          {analyzing ? "Analyzing…" : "Save requirements"}
        </button>
        {onCancel && (
          <button className="overview-digest-row-action" onClick={onCancel}>
            Cancel
          </button>
        )}
        {onClear && (
          <button className="overview-digest-row-action" onClick={onClear}>
            Use PR description instead
          </button>
        )}
      </div>
      <div className="requirements-card-hint">
        Saved locally — coverage re-runs right away, and on every future review.
      </div>
      <button
        className="github-link requirements-card-github-link"
        onClick={() => openUrl(prUrl).catch(() => {})}
      >
        or edit the PR description on GitHub
      </button>
    </>
  );
}

export function RequirementsCard({
  manifest,
  resolvedSpecKeys,
  onResolveSpec,
  onRestoreSpec,
  onOpenAt,
  localRequirements,
  onSaveRequirements,
  analyzing,
}: RequirementsCardProps) {
  const [expandedKey, setExpandedKey] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(localRequirements ?? "");
  const lastLoadedRef = useRef(localRequirements);
  useEffect(() => {
    if (localRequirements !== lastLoadedRef.current) {
      lastLoadedRef.current = localRequirements;
      setDraft(localRequirements ?? "");
    }
  }, [localRequirements]);

  const coverage = manifest.requirements_coverage;
  const hasLocal = !!localRequirements?.trim();

  if (!coverage) {
    return (
      <div className="overview-card requirements-card requirements-card--empty">
        <h4>Requirements</h4>
        <div className="requirements-card-empty-headline">No requirements found for this PR</div>
        <p className="requirements-card-empty-explainer">
          Nothing in the PR description reads as requirements/acceptance criteria —
          coverage can't be judged.
        </p>
        <RequirementsEditor
          draft={draft}
          setDraft={setDraft}
          prUrl={manifest.pr_url}
          analyzing={analyzing}
          onSave={() => onSaveRequirements(draft.trim())}
          onClear={hasLocal ? () => onSaveRequirements("") : undefined}
        />
      </div>
    );
  }

  const { requirements, orphan_tests } = coverage;
  const covered = requirements.filter((r) => r.status === "covered").length;
  const addressedKeys = new Set(
    requirements
      .filter((r) => r.status === "uncovered" || r.status === "partial")
      .map((r) => specResolveKey(r.text))
      .filter((key) => resolvedSpecKeys?.has(key))
  );

  function startEditing() {
    // Local text edits as-is; body-extracted requirements seed the draft as a
    // numbered list, so saving converts them into the local source.
    setDraft(
      hasLocal
        ? (localRequirements ?? "")
        : requirements.map((r, i) => `${i + 1}. ${r.text}`).join("\n")
    );
    setEditing(true);
  }

  if (editing) {
    return (
      <div className="overview-card requirements-card">
        <h4>Requirements</h4>
        <RequirementsEditor
          draft={draft}
          setDraft={setDraft}
          prUrl={manifest.pr_url}
          analyzing={analyzing}
          onSave={() => {
            onSaveRequirements(draft.trim());
            setEditing(false);
          }}
          onCancel={() => setEditing(false)}
          onClear={
            hasLocal
              ? () => {
                  onSaveRequirements("");
                  setEditing(false);
                }
              : undefined
          }
        />
      </div>
    );
  }

  return (
    <div className="overview-card requirements-card">
      <div className="requirements-card-header">
        <h4>Requirements</h4>
        <button className="requirements-card-edit" onClick={startEditing}>
          Edit
        </button>
      </div>
      <div className="requirements-card-progress">
        {covered} of {requirements.length} covered · {addressedKeys.size} addressed
      </div>
      {analyzing && <div className="requirements-card-source">analyzing requirements…</div>}
      {hasLocal ? (
        <div className="requirements-card-source">using your local requirements</div>
      ) : (
        (coverage.source_issues?.length ?? 0) > 0 && (
          <div className="requirements-card-source">
            requirements from linked issue{coverage.source_issues!.length > 1 ? "s" : ""}{" "}
            {coverage.source_issues!.map((n) => `#${n}`).join(", ")}
          </div>
        )
      )}
      {requirements.map((req, i) => {
        const key = `req:${i}`;
        const addressed = addressedKeys.has(specResolveKey(req.text));
        return (
          <RequirementRow
            key={key}
            req={req}
            addressed={addressed}
            expanded={expandedKey === key}
            onToggle={() => setExpandedKey(expandedKey === key ? null : key)}
            onOpenAt={onOpenAt}
            onResolveSpec={onResolveSpec}
            onRestoreSpec={onRestoreSpec}
          />
        );
      })}
      {orphan_tests.length > 0 && (
        <div className="requirements-card-orphans">
          <div className="requirements-card-orphans-label">Tests without a stated requirement</div>
          {orphan_tests.map((test, i) => {
            const key = `orphan:${i}`;
            return (
              <OrphanTestRow
                key={key}
                test={test}
                expanded={expandedKey === key}
                onToggle={() => setExpandedKey(expandedKey === key ? null : key)}
                onOpenAt={onOpenAt}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

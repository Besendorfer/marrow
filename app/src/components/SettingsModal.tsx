import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Settings } from "../types";

// Keep in sync with browser-extension/content.js getPrRef()
const BOOKMARKLET_HREF =
  "javascript:void(function(){var m=window.location.pathname.match(/^\\/([A-Za-z0-9][A-Za-z0-9-]{0,38}\\/[A-Za-z0-9._-]{1,100}\\/pull\\/\\d+)/);if(m){window.location='relevantreviews://github.com/'+m[1]}else{alert('Not a GitHub PR page')}}())";

interface SettingsModalProps {
  open: boolean;
  onClose: () => void;
}

function FeatureToggle({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="feature-toggle">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="feature-toggle-text">
        <span className="feature-toggle-label">{label}</span>
        <span className="feature-toggle-hint">{hint}</span>
      </span>
    </label>
  );
}

export function SettingsModal({ open, onClose }: SettingsModalProps) {
  const [model, setModel] = useState("");
  const [githubToken, setGithubToken] = useState("");
  const [awsProfile, setAwsProfile] = useState("");
  const [currentSettings, setCurrentSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [enableAiSummary, setEnableAiSummary] = useState(true);
  const [enableChangeGroups, setEnableChangeGroups] = useState(true);
  const [enableCommentsView, setEnableCommentsView] = useState(true);
  const [enableChecksStatus, setEnableChecksStatus] = useState(true);
  const [showAiNotes, setShowAiNotes] = useState(true);

  // Set href via DOM to bypass React's javascript: URL blocking
  const bookmarkletRef = useCallback((node: HTMLAnchorElement | null) => {
    if (node) node.setAttribute("href", BOOKMARKLET_HREF);
  }, []);

  useEffect(() => {
    if (open) {
      invoke<Settings>("get_settings").then((s) => {
        setModel(s.model);
        setGithubToken(s.github_token || "");
        setAwsProfile(s.aws_profile || "");
        setEnableAiSummary(s.enable_ai_summary ?? true);
        setEnableChangeGroups(s.enable_change_groups ?? true);
        setEnableCommentsView(s.enable_comments_view ?? true);
        setEnableChecksStatus(s.enable_checks_status ?? true);
        setShowAiNotes(s.show_ai_notes ?? true);
        setCurrentSettings(s);
      });
      setSaved(false);
    }
  }, [open]);

  async function handleSave(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      await invoke("save_settings", {
        settings: {
          model: model.trim(),
          github_token: githubToken.trim(),
          aws_profile: awsProfile.trim(),
          filter_older: currentSettings?.filter_older ?? true,
          filter_team: currentSettings?.filter_team ?? true,
          view_mode: currentSettings?.view_mode ?? "split",
          show_hunk_significance: currentSettings?.show_hunk_significance ?? true,
          show_ai_notes: showAiNotes,
          hunk_filter: currentSettings?.hunk_filter ?? "all",
          enable_ai_summary: enableAiSummary,
          enable_change_groups: enableChangeGroups,
          enable_comments_view: enableCommentsView,
          enable_checks_status: enableChecksStatus,
        },
      });
      setSaved(true);
      setTimeout(() => onClose(), 600);
    } finally {
      setSaving(false);
    }
  }

  if (!open) return null;

  return (
    <div className="settings-overlay" onClick={onClose}>
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        <div className="settings-header">
          <h2>Settings</h2>
          <button className="settings-close" onClick={onClose} aria-label="Close settings">
            &times;
          </button>
        </div>
        <form onSubmit={handleSave}>
          <label className="settings-label" htmlFor="model">
            Claude Model
          </label>
          <p className="settings-hint">
            A Claude model name (e.g. <code>claude-sonnet-4-6</code>) to use
            via the <code>claude</code> CLI, or a Bedrock ARN for AWS.
          </p>
          <input
            id="model"
            className="settings-input"
            type="text"
            value={model}
            onChange={(e) => {
              setModel(e.target.value);
              setSaved(false);
            }}
            placeholder="claude-sonnet-4-6 or arn:aws:bedrock:..."
            spellCheck={false}
          />

          {model.trim().startsWith("arn:") && (
            <>
              <label className="settings-label" htmlFor="aws-profile">
                AWS Profile
              </label>
              <p className="settings-hint">
                The AWS profile name from <code>~/.aws/config</code> to use
                for Bedrock authentication (e.g.{" "}
                <code>claude-code-bedrock</code>). Run{" "}
                <code>aws sso login --profile &lt;name&gt;</code> to refresh
                credentials.
              </p>
              <input
                id="aws-profile"
                className="settings-input"
                type="text"
                value={awsProfile}
                onChange={(e) => {
                  setAwsProfile(e.target.value);
                  setSaved(false);
                }}
                placeholder="default"
                spellCheck={false}
              />
            </>
          )}

          <label className="settings-label" htmlFor="github-token">
            GitHub Token
          </label>
          <p className="settings-hint">
            Personal access token for GitHub API. Needs <code>repo</code> scope
            for private repos. Falls back to <code>GH_TOKEN</code> /{" "}
            <code>GITHUB_TOKEN</code> env vars.
          </p>
          <input
            id="github-token"
            className="settings-input"
            type="password"
            value={githubToken}
            onChange={(e) => {
              setGithubToken(e.target.value);
              setSaved(false);
            }}
            placeholder="ghp_... or github_pat_..."
            spellCheck={false}
            autoComplete="off"
          />

          <div className="settings-divider" />
          <h3 className="settings-section-title">Features</h3>
          <p className="settings-hint">
            Turn off features you don't use to declutter the UI. Items marked
            <em> (skips AI work)</em> also avoid the related AI calls during
            fetch, saving cost and time.
          </p>
          <FeatureToggle
            label="AI highlights"
            hint="Inline AI annotations on the diff. (skips AI work)"
            checked={showAiNotes}
            onChange={(v) => { setShowAiNotes(v); setSaved(false); }}
          />
          <FeatureToggle
            label="AI summary"
            hint="The PR-level summary shown when no file is selected. (skips AI work)"
            checked={enableAiSummary}
            onChange={(v) => { setEnableAiSummary(v); setSaved(false); }}
          />
          <FeatureToggle
            label="Change groups"
            hint="The Groups sidebar view that clusters related files. (skips AI work)"
            checked={enableChangeGroups}
            onChange={(v) => { setEnableChangeGroups(v); setSaved(false); }}
          />
          <FeatureToggle
            label="Comments view"
            hint="The Comments sidebar view and review-thread fetching."
            checked={enableCommentsView}
            onChange={(v) => { setEnableCommentsView(v); setSaved(false); }}
          />
          <FeatureToggle
            label="CI checks status"
            hint="The blocking modal and live polling for failing/pending checks."
            checked={enableChecksStatus}
            onChange={(v) => { setEnableChecksStatus(v); setSaved(false); }}
          />

          <div className="settings-divider" />
          <h3 className="settings-section-title">Browser Integration</h3>
          <p className="settings-hint">
            Drag this link to your bookmark bar. When you're on a GitHub PR
            page, click it to open the PR directly in Relevant Reviews.
          </p>
          <a
            className="bookmarklet-link"
            ref={bookmarkletRef}
            onClick={(e) => e.preventDefault()}
            draggable
          >
            Open in Relevant Reviews
          </a>

          <div className="settings-actions">
            <button
              type="button"
              className="settings-cancel"
              onClick={onClose}
            >
              Cancel
            </button>
            <button
              type="submit"
              className="settings-save"
              disabled={saving || !model.trim()}
            >
              {saved ? "Saved" : saving ? "Saving..." : "Save"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

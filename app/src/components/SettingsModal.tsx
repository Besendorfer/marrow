import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Settings } from "../types";

// Keep in sync with browser-extension/content.js getPrRef()
const BOOKMARKLET_HREF =
  "javascript:void(function(){var m=window.location.pathname.match(/^\\/([^/]+\\/[^/]+\\/pull\\/\\d+)/);if(m){window.location='relevantreviews://github.com/'+m[1]}else{alert('Not a GitHub PR page')}}())";

interface SettingsModalProps {
  open: boolean;
  onClose: () => void;
}

export function SettingsModal({ open, onClose }: SettingsModalProps) {
  const [model, setModel] = useState("");
  const [githubToken, setGithubToken] = useState("");
  const [awsProfile, setAwsProfile] = useState("");
  const [currentSettings, setCurrentSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [interceptEnabled, setInterceptEnabled] = useState(false);
  const [interceptLoading, setInterceptLoading] = useState(false);

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
        setCurrentSettings(s);
      });
      invoke<string | null>("get_fallback_browser").then((b) => {
        setInterceptEnabled(!!b);
      });
      setSaved(false);
    }
  }, [open]);

  async function handleEnableIntercept() {
    setInterceptLoading(true);
    try {
      const currentBrowser = await invoke<string | null>("detect_default_browser");
      const fallback = currentBrowser || "com.apple.safari";
      await invoke("enable_browser_intercept", { fallbackBrowser: fallback });
      await invoke("open_default_browser_settings");
      setInterceptEnabled(true);
    } finally {
      setInterceptLoading(false);
    }
  }

  async function handleDisableIntercept() {
    setInterceptLoading(true);
    try {
      await invoke("disable_browser_intercept");
      await invoke("open_default_browser_settings");
      setInterceptEnabled(false);
    } finally {
      setInterceptLoading(false);
    }
  }

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
          show_ai_notes: currentSettings?.show_ai_notes ?? true,
          hunk_filter: currentSettings?.hunk_filter ?? "all",
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

          <div className="settings-divider" />
          <h3 className="settings-section-title">Link Interception</h3>
          <p className="settings-hint">
            Set Relevant Reviews as your default browser so GitHub PR links from
            Slack, email, or anywhere open directly in the app. All other links
            are forwarded to your real browser.
          </p>
          {interceptEnabled ? (
            <div className="intercept-status">
              <span className="intercept-badge intercept-badge--active">Active</span>
              <button
                type="button"
                className="intercept-btn"
                onClick={handleDisableIntercept}
                disabled={interceptLoading}
              >
                Disable
              </button>
              <p className="settings-hint" style={{ marginTop: 8 }}>
                Select your preferred browser in System Settings to stop
                intercepting links.
              </p>
            </div>
          ) : (
            <div className="intercept-status">
              <button
                type="button"
                className="intercept-btn intercept-btn--enable"
                onClick={handleEnableIntercept}
                disabled={interceptLoading}
              >
                {interceptLoading ? "Opening Settings..." : "Enable Link Interception"}
              </button>
              <p className="settings-hint" style={{ marginTop: 8 }}>
                Opens System Settings. Select <strong>Relevant Reviews</strong> as
                your default web browser.
              </p>
            </div>
          )}

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

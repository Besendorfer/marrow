import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Settings, Watch } from "../types";

function newWatchId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `w-${Date.now()}-${Math.floor(performance.now())}`;
}

// Keep in sync with browser-extension/content.js getPrRef()
const BOOKMARKLET_HREF =
  "javascript:void(function(){var m=window.location.pathname.match(/^\\/([A-Za-z0-9][A-Za-z0-9-]{0,38}\\/[A-Za-z0-9._-]{1,100}\\/pull\\/\\d+)/);if(m){window.location='relevantreviews://github.com/'+m[1]}else{alert('Not a GitHub PR page')}}())";

interface SettingsModalProps {
  open: boolean;
  onClose: () => void;
}

export function SettingsModal({ open, onClose }: SettingsModalProps) {
  const [model, setModel] = useState("");
  const [githubToken, setGithubToken] = useState("");
  const [awsProfile, setAwsProfile] = useState("");
  const [anthropicKey, setAnthropicKey] = useState("");
  const [openaiKey, setOpenaiKey] = useState("");
  const [geminiKey, setGeminiKey] = useState("");
  const [provider, setProvider] = useState("");
  const [openaiBaseUrl, setOpenaiBaseUrl] = useState("");
  const [watches, setWatches] = useState<Watch[]>([]);
  const [perWatchCap, setPerWatchCap] = useState(50);
  const [showApprovedPrs, setShowApprovedPrs] = useState(false);
  const [expandAllHunks, setExpandAllHunks] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

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
        setAnthropicKey(s.anthropic_api_key || "");
        setOpenaiKey(s.openai_api_key || "");
        setGeminiKey(s.gemini_api_key || "");
        setProvider(s.provider || "");
        setOpenaiBaseUrl(s.openai_base_url || "");
        setPerWatchCap(s.activity_per_watch_cap || 50);
        setShowApprovedPrs(s.show_approved_prs ?? false);
        setExpandAllHunks(s.expand_all_hunks ?? true);
      });
      invoke<Watch[]>("get_watches").then(setWatches).catch(() => {});
      setSaved(false);
    }
  }, [open]);

  function updateWatch(id: string, field: "label" | "query", value: string) {
    setWatches((ws) => ws.map((w) => (w.id === id ? { ...w, [field]: value } : w)));
    setSaved(false);
  }
  function addWatch() {
    setWatches((ws) => [...ws, { id: newWatchId(), label: "", query: "" }]);
    setSaved(false);
  }
  function removeWatch(id: string) {
    setWatches((ws) => ws.filter((w) => w.id !== id));
    setSaved(false);
  }

  async function handleSave(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      // Re-read fresh and override only the fields this modal edits, so settings
      // changed elsewhere since the modal opened (e.g. activity_mini_player via
      // the dock toggle or the floating ✕) aren't clobbered by a stale snapshot.
      const fresh = await invoke<Settings>("get_settings");
      await invoke("save_settings", {
        settings: {
          ...fresh,
          model: model.trim(),
          github_token: githubToken.trim(),
          aws_profile: awsProfile.trim(),
          anthropic_api_key: anthropicKey.trim(),
          openai_api_key: openaiKey.trim(),
          gemini_api_key: geminiKey.trim(),
          provider: provider.trim(),
          openai_base_url: openaiBaseUrl.trim(),
          activity_per_watch_cap: perWatchCap,
          show_approved_prs: showApprovedPrs,
          expand_all_hunks: expandAllHunks,
        },
      });
      // Persist watches alongside settings, dropping blank rows.
      await invoke("save_watches", {
        watches: watches
          .map((w) => ({ ...w, label: w.label.trim(), query: w.query.trim() }))
          .filter((w) => w.query !== ""),
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
            A model name — the provider is auto-detected:{" "}
            <code>claude*</code> → Anthropic, <code>gpt*</code>/<code>o3*</code>{" "}
            → OpenAI, <code>gemini*</code> → Gemini. Or a Bedrock ARN for AWS.
            Set the matching API key below.
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

          {!model.trim().startsWith("arn:") && (
            <>
              <label className="settings-label" htmlFor="anthropic-key">
                Anthropic API Key
              </label>
              <p className="settings-hint">
                For <code>claude*</code> models — calls the Anthropic API
                directly (no AWS or <code>claude</code> CLI needed). Falls back to{" "}
                <code>ANTHROPIC_API_KEY</code>, then the <code>claude</code> CLI.
              </p>
              <input
                id="anthropic-key"
                className="settings-input"
                type="password"
                value={anthropicKey}
                onChange={(e) => {
                  setAnthropicKey(e.target.value);
                  setSaved(false);
                }}
                placeholder="sk-ant-..."
                spellCheck={false}
                autoComplete="off"
              />

              <label className="settings-label" htmlFor="openai-key">
                OpenAI API Key
              </label>
              <p className="settings-hint">
                For <code>gpt*</code> / <code>o3*</code> models (or{" "}
                <code>OPENAI_API_KEY</code>). Also used for OpenAI-compatible
                endpoints below.
              </p>
              <input
                id="openai-key"
                className="settings-input"
                type="password"
                value={openaiKey}
                onChange={(e) => {
                  setOpenaiKey(e.target.value);
                  setSaved(false);
                }}
                placeholder="sk-..."
                spellCheck={false}
                autoComplete="off"
              />

              <label className="settings-label" htmlFor="gemini-key">
                Gemini API Key
              </label>
              <p className="settings-hint">
                For <code>gemini*</code> models (or <code>GEMINI_API_KEY</code>).
              </p>
              <input
                id="gemini-key"
                className="settings-input"
                type="password"
                value={geminiKey}
                onChange={(e) => {
                  setGeminiKey(e.target.value);
                  setSaved(false);
                }}
                placeholder="AIza..."
                spellCheck={false}
                autoComplete="off"
              />

              <label className="settings-label" htmlFor="provider">
                Provider override
              </label>
              <p className="settings-hint">
                Optional — leave blank to auto-detect from the model name. Set{" "}
                <code>openai-compatible</code> for OpenRouter or a local server,
                then fill in the base URL + OpenAI key.
              </p>
              <input
                id="provider"
                className="settings-input"
                type="text"
                value={provider}
                onChange={(e) => {
                  setProvider(e.target.value);
                  setSaved(false);
                }}
                placeholder="(auto) · openai · gemini · openai-compatible"
                spellCheck={false}
              />

              <label className="settings-label" htmlFor="openai-base-url">
                OpenAI-compatible Base URL
              </label>
              <p className="settings-hint">
                Optional — point at OpenRouter, a local server, etc. (or{" "}
                <code>OPENAI_BASE_URL</code>). Setting it routes through the
                OpenAI-compatible backend.
              </p>
              <input
                id="openai-base-url"
                className="settings-input"
                type="text"
                value={openaiBaseUrl}
                onChange={(e) => {
                  setOpenaiBaseUrl(e.target.value);
                  setSaved(false);
                }}
                placeholder="https://openrouter.ai/api/v1"
                spellCheck={false}
              />
            </>
          )}

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
          <h3 className="settings-section-title">PR Activity Watches</h3>
          <p className="settings-hint">
            Saved GitHub searches that feed the activity mini-player — including
            repos/orgs where you aren't a requested reviewer. Use GitHub search
            syntax, e.g. <code>is:pr is:open repo:acme/web -is:draft</code>.
          </p>
          <div className="watch-editor">
            {watches.map((w) => (
              <div className="watch-row" key={w.id}>
                <input
                  className="settings-input watch-row__label"
                  type="text"
                  value={w.label}
                  onChange={(e) => updateWatch(w.id, "label", e.target.value)}
                  placeholder="Label"
                  spellCheck={false}
                />
                <input
                  className="settings-input watch-row__query"
                  type="text"
                  value={w.query}
                  onChange={(e) => updateWatch(w.id, "query", e.target.value)}
                  placeholder="is:pr is:open repo:owner/name"
                  spellCheck={false}
                />
                <button
                  type="button"
                  className="watch-row__remove"
                  onClick={() => removeWatch(w.id)}
                  aria-label="Remove watch"
                >
                  &times;
                </button>
              </div>
            ))}
            <button type="button" className="watch-add" onClick={addWatch}>
              + Add watch
            </button>
          </div>

          <label className="settings-label" htmlFor="per-watch-cap">
            Max PRs per watch
          </label>
          <p className="settings-hint">
            How many PRs each watch surfaces in the mini-player; the rest show as
            "+N more". Raise this for org-wide watches that match many PRs.
          </p>
          <input
            id="per-watch-cap"
            className="settings-input"
            type="number"
            min={1}
            max={200}
            value={perWatchCap}
            onChange={(e) => {
              setPerWatchCap(Math.max(1, Number(e.target.value) || 1));
              setSaved(false);
            }}
          />

          <label className="settings-check">
            <input
              type="checkbox"
              checked={showApprovedPrs}
              onChange={(e) => {
                setShowApprovedPrs(e.target.checked);
                setSaved(false);
              }}
            />
            Show PRs I've approved
          </label>
          <p className="settings-hint">
            By default, approving a PR removes it from the mini-player feed.
            Enable this to keep approved PRs in the list.
          </p>
          <h3 className="settings-section-title">Review display</h3>
          <label className="settings-checkbox">
            <input
              type="checkbox"
              checked={expandAllHunks}
              onChange={(e) => {
                setExpandAllHunks(e.target.checked);
                setSaved(false);
              }}
            />
            Expand all hunks by default
          </label>
          <p className="settings-hint">
            When off, low-significance hunks start collapsed so you can focus on the
            changes that matter (takes effect the next time you open a file).
          </p>

          <div className="settings-divider" />
          <h3 className="settings-section-title">Browser Integration</h3>
          <p className="settings-hint">
            Drag this link to your bookmark bar. When you're on a GitHub PR
            page, click it to open the PR directly in Marrow.
          </p>
          <a
            className="bookmarklet-link"
            ref={bookmarkletRef}
            onClick={(e) => e.preventDefault()}
            draggable
          >
            Open in Marrow
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

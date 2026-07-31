import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Settings } from "../types";

interface WelcomeSetupProps {
  onDone: () => void;
  onOpenSettings: () => void;
}

type CheckState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "ok"; detail: string }
  | { status: "error"; message: string };

// Two-step first-run setup. Each step validates against the live service
// before moving on, so a bad token or key can't fail thirty seconds into the
// first PR fetch. Skippable — config files and env vars still work.
export function WelcomeSetup({ onDone, onOpenSettings }: WelcomeSetupProps) {
  const [step, setStep] = useState<1 | 2>(1);
  const [token, setToken] = useState("");
  const [tokenCheck, setTokenCheck] = useState<CheckState>({ status: "idle" });
  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [aiCheck, setAiCheck] = useState<CheckState>({ status: "idle" });
  const [saving, setSaving] = useState(false);

  async function checkToken() {
    setTokenCheck({ status: "checking" });
    try {
      const login = await invoke<string>("validate_github_token", { token });
      setTokenCheck({ status: "ok", detail: login });
    } catch (e) {
      setTokenCheck({ status: "error", message: String(e) });
    }
  }

  async function mergedSettings(): Promise<Settings> {
    const current = await invoke<Settings>("get_settings");
    return {
      ...current,
      github_token: token.trim() || current.github_token,
      model: model.trim() || current.model,
      anthropic_api_key: apiKey.trim() || current.anthropic_api_key,
      setup_done: true,
    };
  }

  async function checkAi() {
    setAiCheck({ status: "checking" });
    try {
      const candidate = await mergedSettings();
      const provider = await invoke<string>("validate_ai_provider", { settings: candidate });
      setAiCheck({ status: "ok", detail: provider });
    } catch (e) {
      setAiCheck({ status: "error", message: String(e) });
    }
  }

  async function finish(markDone: boolean) {
    setSaving(true);
    try {
      const settings = markDone
        ? await mergedSettings()
        : { ...(await invoke<Settings>("get_settings")), setup_done: true };
      await invoke("save_settings", { settings });
    } catch {
      // Saving is best-effort here — worst case the welcome shows again.
    } finally {
      setSaving(false);
      onDone();
    }
  }

  return (
    <div className="settings-overlay">
      <div className="welcome-card">
        {step === 1 ? (
          <>
            <div className="welcome-step">Welcome to Marrow · Step 1 of 2</div>
            <h3>Connect GitHub</h3>
            <p>
              Marrow reads pull requests with a personal access token (classic
              or fine-grained, with repo read access). It stays on this Mac.
            </p>
            <label className="welcome-label">Personal access token</label>
            <input
              className="settings-input"
              type="password"
              value={token}
              onChange={(e) => {
                setToken(e.target.value);
                setTokenCheck({ status: "idle" });
              }}
              placeholder="ghp_… or github_pat_…"
              autoFocus
            />
            {tokenCheck.status === "ok" ? (
              <div className="welcome-check welcome-check--ok">
                ✓ Connected as @{tokenCheck.detail}
              </div>
            ) : tokenCheck.status === "error" ? (
              <div className="welcome-check welcome-check--err">
                {tokenCheck.message}
              </div>
            ) : null}
            <div className="welcome-actions">
              {tokenCheck.status === "ok" ? (
                <button className="welcome-primary" onClick={() => setStep(2)}>
                  Continue → AI provider
                </button>
              ) : (
                <button
                  className="welcome-primary"
                  onClick={checkToken}
                  disabled={!token.trim() || tokenCheck.status === "checking"}
                >
                  {tokenCheck.status === "checking" ? "Checking…" : "Test connection"}
                </button>
              )}
              <button className="welcome-skip" onClick={() => finish(false)} disabled={saving}>
                I'll do this later
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="welcome-step">Welcome to Marrow · Step 2 of 2</div>
            <h3>Choose an AI provider</h3>
            <p>
              The AI finds the files and lines worth your attention. Enter a
              model and key — or leave both empty to use the local{" "}
              <code>claude</code> CLI if it's installed.
            </p>
            <label className="welcome-label">Model</label>
            <input
              className="settings-input"
              type="text"
              value={model}
              onChange={(e) => {
                setModel(e.target.value);
                setAiCheck({ status: "idle" });
              }}
              placeholder="e.g. claude-sonnet-4-5, gpt-5.2, gemini-3-pro"
            />
            <label className="welcome-label">Anthropic API key (for claude-* models)</label>
            <input
              className="settings-input"
              type="password"
              value={apiKey}
              onChange={(e) => {
                setApiKey(e.target.value);
                setAiCheck({ status: "idle" });
              }}
              placeholder="sk-ant-… (leave empty for the claude CLI)"
            />
            <div className="welcome-hint">
              OpenAI, Gemini, Bedrock, and custom endpoints are configurable in{" "}
              <button className="welcome-link" onClick={() => { onDone(); onOpenSettings(); }}>
                Settings
              </button>
              .
            </div>
            {aiCheck.status === "ok" ? (
              <div className="welcome-check welcome-check--ok">
                ✓ Provider responded ({aiCheck.detail})
              </div>
            ) : aiCheck.status === "error" ? (
              <div className="welcome-check welcome-check--err">{aiCheck.message}</div>
            ) : null}
            <div className="welcome-actions">
              {aiCheck.status === "ok" ? (
                <button className="welcome-primary" onClick={() => finish(true)} disabled={saving}>
                  Finish setup
                </button>
              ) : (
                <button
                  className="welcome-primary"
                  onClick={checkAi}
                  disabled={aiCheck.status === "checking"}
                >
                  {aiCheck.status === "checking" ? "Checking…" : "Test provider"}
                </button>
              )}
              {aiCheck.status !== "ok" && (
                <button className="welcome-skip" onClick={() => finish(true)} disabled={saving}>
                  Skip — finish anyway
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

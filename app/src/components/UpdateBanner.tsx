import type { UpdateStatus } from "../types";

export function UpdateBanner({
  status,
  onDownload,
  onRelaunch,
  onDismiss,
}: {
  status: UpdateStatus;
  onDownload: () => void;
  onRelaunch: () => void;
  onDismiss: () => void;
}) {
  if (status.state === "idle" || status.state === "up-to-date") return null;

  const dismissible = status.state === "available" || status.state === "ready";

  return (
    <div className="update-banner">
      {status.state === "checking" && (
        <span className="update-banner-text">Checking for updates...</span>
      )}
      {status.state === "available" && (
        <>
          <span className="update-banner-text">
            Version {status.version} is available
          </span>
          <button className="update-banner-action" onClick={onDownload}>
            Download &amp; Install
          </button>
        </>
      )}
      {status.state === "downloading" && (
        <>
          <span className="update-banner-text">Downloading update...</span>
          <div className="update-banner-progress">
            <div
              className="update-banner-progress-fill"
              style={{ width: `${status.progress}%` }}
            />
          </div>
          <span className="update-banner-pct">{status.progress}%</span>
        </>
      )}
      {status.state === "ready" && (
        <>
          <span className="update-banner-text">
            Update installed. Relaunch to apply.
          </span>
          <button className="update-banner-action" onClick={onRelaunch}>
            Relaunch
          </button>
        </>
      )}
      {dismissible && (
        <button className="update-banner-dismiss" onClick={onDismiss} aria-label="Dismiss update notification">
          &times;
        </button>
      )}
    </div>
  );
}

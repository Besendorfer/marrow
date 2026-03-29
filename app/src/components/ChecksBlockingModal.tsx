import type { PrChecksStatus } from "../types";

interface ChecksBlockingModalProps {
  checksStatus: PrChecksStatus;
  onDismiss: () => void;
}

export function ChecksBlockingModal({ checksStatus, onDismiss }: ChecksBlockingModalProps) {
  const isPending = checksStatus.overall_state === "pending";
  const isFailing = checksStatus.overall_state === "failure";

  const failingChecks = checksStatus.check_runs.filter(
    (c) => c.conclusion === "FAILURE" || c.conclusion === "CANCELLED" || c.conclusion === "TIMED_OUT"
  );
  const pendingChecks = checksStatus.check_runs.filter(
    (c) => c.status !== "COMPLETED"
  );

  return (
    <div className="checks-modal-overlay">
      <div className="checks-modal">
        <div className="checks-modal-icon">
          {isPending ? (
            <span className="checks-modal-spinner" />
          ) : (
            <span className="checks-modal-x">&times;</span>
          )}
        </div>
        <h2 className="checks-modal-title">
          {isPending ? "Checks are running" : "Checks are failing"}
        </h2>
        <p className="checks-modal-description">
          {isPending
            ? "CI checks are still in progress. The review will be unblocked automatically when all checks pass."
            : "One or more CI checks have failed. You may want to wait for fixes before reviewing."}
        </p>

        {failingChecks.length > 0 && (
          <div className="checks-modal-list">
            {failingChecks.map((check) => (
              <div key={check.name} className="checks-modal-item checks-modal-item-fail">
                <span className="checks-modal-item-icon">&times;</span>
                <span className="checks-modal-item-name">{check.name}</span>
              </div>
            ))}
          </div>
        )}

        {pendingChecks.length > 0 && (
          <div className="checks-modal-list">
            {pendingChecks.map((check) => (
              <div key={check.name} className="checks-modal-item checks-modal-item-pending">
                <span className="checks-modal-item-icon checks-modal-dot-pulse" />
                <span className="checks-modal-item-name">{check.name}</span>
              </div>
            ))}
          </div>
        )}

        {isFailing && failingChecks.length === 0 && pendingChecks.length === 0 && (
          <div className="checks-modal-list">
            <div className="checks-modal-item checks-modal-item-fail">
              <span className="checks-modal-item-icon">&times;</span>
              <span className="checks-modal-item-name">One or more checks failed</span>
            </div>
          </div>
        )}

        <button className="checks-modal-dismiss" onClick={onDismiss}>
          Ignore
        </button>
      </div>
    </div>
  );
}
